# Tutorial: a real-time chat app

You'll build a desktop chat app where two instances on one machine (or two
machines) message each other live, with message history that survives
restarts. Live delivery uses gossipsub pub/sub — `Runtime`'s `network`
field (`NetworkManager`) gives you `subscribe(topic)` → a stream of frames
and `publish(topic, data)` — and history is a JSON blob stored as a signed
object, shared under a name so it can be resolved again later. Everything
runs inside the app process: no `canopee-node`, no server.

This tutorial assumes [tauri-quickstart.md](tauri-quickstart.md) is done
and you have a managed `Arc<Runtime>` plus the same `Cargo.toml`
dependencies. Concepts: [Networking](../concepts/networking.md) for
pub/sub, [Objects & pointers](../concepts/objects.md) for history.

## 1. Pick a topic for the conversation

A gossipsub topic is a shared string; anyone who knows it can subscribe.
For a room-based chat, a public room name is fine. For a 1:1 conversation,
derive the topic from both identities so the string itself isn't guessable
(e.g. hex of a hash of the two sorted `IdentityId`s), and include your
identity in every message payload anyway — the app shows *who* spoke, and
the peer's device key signs the frame so sources within the mesh can't be
spoofed (see [Identity](../concepts/identity.md) for the device-vs-person
distinction).

```rust
fn room_topic(room: &str) -> String {
    format!("chat-{room}")
}
```

**Verify it works:** two app instances can each compute the same topic
string for the same room — no network needed yet.

## 2. Subscribe and forward incoming messages to the frontend

When the app starts (or the user joins a room), subscribe and spawn a
background task that forwards every received frame into a Tauri event:

```rust
use tauri::Emitter;
use serde::Serialize;
use std::sync::Arc;
use canopee_runtime::Runtime;

#[derive(Clone, Serialize)]
struct ChatMsg {
    room: String,
    identity: String,
    text: String,
}

// run once at setup, after the Runtime is managed:
fn spawn_chat_listener(app: tauri::AppHandle, runtime: Arc<Runtime>, room: String) {
    tauri::async_runtime::spawn(async move {
        let mut rx = match runtime.network.subscribe(&room_topic(&room)).await {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("subscribe failed: {e}");
                return;
            }
        };
        while let Ok(frame) = rx.recv().await {
            // frame: canopee_network::message::PubSubMessage { topic, source, data }
            let Ok(msg) = serde_json::from_slice::<ChatMsg>(&frame.data) else {
                continue;
            };
            if msg.identity == runtime.identity().id().to_string() {
                continue; // echo of our own publish
            }
            let _ = app.emit("chat-message", &msg);
        }
    });
}
```

Frontend side:

```ts
import { listen } from "@tauri-apps/api/event";

listen<{ room: string; identity: string; text: string }>("chat-message", (e) => {
  appendMessage(e.payload); // push into the messages list in the UI
});
```

**Verify it works:** with two instances joined to the same room, a frame
published by one (manually, via a debug `publish` command) is delivered to
the other's `chat-message` event within a second.

## 3. A `send_message` command that publishes

Sending is one call: `runtime.network.publish(topic, bytes)`. Serialize the
message (identity + text) with `serde_json` and publish. The identity string
is read straight from the embedded runtime, so a message can never claim a
wrong sender by accident.

```rust
#[tauri::command]
async fn send_message(
    runtime: State<'_, Arc<Runtime>>,
    room: String,
    text: String,
) -> Result<(), String> {
    let identity = runtime.identity().id().to_string();
    let msg = ChatMsg { room: room.clone(), identity, text };
    let bytes = serde_json::to_vec(&msg).map_err(|e| e.to_string())?;
    runtime.network.publish(&room_topic(&room), bytes).await.map_err(|e| e.to_string())
}
```

```ts
function send() {
  invoke("send_message", { room, text: input.value });
}
```

**Verify it works:** type on one instance; it appears on the other, live,
tagged with the sender's `canopee://identity/...` string.

## 4. History: a JSON snapshot, stored and shared under a name

Gossipsub is ephemeral — nothing is delivered to peers who weren't
subscribed. For history, serialize the chat log to JSON and store it as a
signed object, then share it under a name so it resolves by
`(owner, "history")`. Every so often (every 10 messages, or a debounce
timer) write a new snapshot and repoint the pointer:

```rust
#[tauri::command]
async fn save_history(
    runtime: State<'_, Arc<Runtime>>,
    room: String,
    messages: Vec<ChatMsg>,
) -> Result<(), String> {
    let key = format!("history:{room}");
    let bytes = serde_json::to_vec(&messages).map_err(|e| e.to_string())?;
    let obj = runtime
        .put_object(bytes, ObjectType::Blob, None)
        .await
        .map_err(|e| e.to_string())?;
    runtime.share_object(&key, &obj.id, None).await.map_err(|e| e.to_string())
}
```

Loading on join resolves your own `(owner, key)` pointer — write and read on
the same identity, so the pointer is signed by you and always verifies:

```rust
#[tauri::command]
async fn load_history(
    runtime: State<'_, Arc<Runtime>>,
    room: String,
) -> Result<Vec<ChatMsg>, String> {
    let owner = runtime.identity().id().clone();
    let key = format!("history:{room}");
    let Some(record) = runtime.resolve_pointer(&owner, &key).await.map_err(|e| e.to_string())?
    else {
        return Ok(vec![]);
    };
    let object = runtime.get(&record.manifest).await.map_err(|e| e.to_string())?;
    Ok(serde_json::from_slice(&object.payload.data).unwrap_or_default())
}
```

Sharing also means a *different* identity — your pair, or another device of
yours — can fetch the same history by resolving your `(owner, key)` pointer
and pulling the object from any provider (see
[Networking](../concepts/networking.md)).

**Verify it works:** exchange a few messages, quit both instances, relaunch
one, and confirm `load_history` returns the exchanged messages — restored
from the stored object, not from gossip (which carries nothing after the
fact).

## 5. Pulling it together

On join: `load_history`, then `spawn_chat_listener`; on send:
`send_message`, then append locally; on `chat-message` event: append and
mark the history dirty; when the dirty count grows past a threshold: call
`save_history`. That's a complete, networked chat app whose only moving
part on the machine is the app itself.

Live frames are ephemeral and unaddressed — scoped to peers in the topic's
mesh, which is exactly why Step 4's shared object is the durable half.
The same "ephemeral pub/sub + durable object pointer" pairing powers the
[collab editor](tauri-collab-editor.md) and the
[multiplayer game](tauri-multiplayer-game.md) tutorials.

**Verify it works (end to end):** a fresh instance joins, restores history,
and can carry on the conversation live with an instance that never left.