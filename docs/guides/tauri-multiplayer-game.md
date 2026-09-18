# Tutorial: a multiplayer game — shared live state, ephemeral messaging

You'll build a desktop multiplayer game where each client streams its
position over a shared gossipsub topic, and every client renders the other
players' positions live. Game state is **ephemeral by design**: it lives in
pub/sub frames, is never stored as objects, and starts fresh every session —
perfect for a small casual game (racing, a shared arena, a co-op clicker).

Foundation: [tauri-quickstart.md](tauri-quickstart.md) (managed
`Arc<Runtime>`). The personality detail matters here: use
`Config::with_root(app_data_dir)` (full isolation) so *each player's install
has its own identity* — players are distinct people, not one person's
devices. Concepts: [Networking](../concepts/networking.md) for pub/sub.

## 1. The room topic and the position message

Every player joins the same room by subscribing to `game-room-<room>` —
a topic string both sides know (two players pick a room name, or it's woven
into a match invite). The payload is a small JSON message that tags the
sender with their identity:

```rust
use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
struct PositionMsg {
    player: String, // runtime.identity().id() — the *person*, stable across
    x: f32,         // their devices, so reconnects keep identity
    y: f32,
    seq: u64,       // monotonic per player; used to discard stale frames
}

fn room_topic(room: &str) -> String {
    format!("game-room-{room}")
}
```

**Verify it works:** both clients compute the same `game-room-<room>` string
for the same room name.

## 2. Subscribe and forward other players' positions

On join, subscribe and spawn a listener that forwards every position frame
into a Tauri event, dropping our own echoes:

```rust
fn spawn_room_listener(app: tauri::AppHandle, runtime: Arc<Runtime>, room: String) {
    tauri::async_runtime::spawn(async move {
        let mut rx = match runtime.network.subscribe(&room_topic(&room)).await {
            Ok(rx) => rx,
            Err(e) => { eprintln!("subscribe failed: {e}"); return; }
        };
        let me = runtime.identity().id().to_string();
        while let Ok(frame) = rx.recv().await {
            let Ok(msg) = serde_json::from_slice::<PositionMsg>(&frame.data) else { continue };
            if msg.player == me { continue; }
            let _ = app.emit("player-position", &msg);
        }
    });
}
```

```ts
import { listen } from "@tauri-apps/api/event";
listen<{ player: string; x: number; y: number; seq: number }>(
  "player-position",
  (e) => upsertPlayer(e.payload) // render/move that sprite on the canvas
);
```

**Verify it works:** publish an arbitrary byte blob to the room from a second
instance; it lands on the other side's `player-position` event (deserialized
or dropped silently).

## 3. Publish our position — throttled and batched

Gossipsub has no rate limit, but your UI refreshes maybe 60×/s while your
network budget is more like 10/s. Throttle: keep one "latest position" and a
publish ticker; at each tick, send the newest one and drop the rest. This
*batches* several input frames per network frame, and it means a slow client
naturally receives the freshest position, not a queue of stale ones.

A clean way to structure it: a background worker owns an `mpsc` channel.
The UI sends `(x, y)` as fast as it likes; the worker sinks it into a
`latest` slot, and a `tokio::time::interval` publishes that slot at most N
times per second. That's all "batched" means here — many local inputs, few
network sends:

```rust
const MAX_PUBLISH_PER_SEC: u64 = 10;

fn spawn_position_publisher(runtime: Arc<Runtime>, room: String, rx: mpsc::Receiver<(f32,f32)>) {
    tauri::async_runtime::spawn(async move {
        let mut rx = rx;
        let mut latest: Option<(f32, f32)> = None;
        let mut seq: u64 = 0;
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(1000 / MAX_PUBLISH_PER_SEC));
        loop {
            tokio::select! {
                Some((x, y)) = rx.recv() => latest = Some((x, y)),
                _ = ticker.tick() => {
                    if let Some((x, y)) = latest.take() {
                        seq += 1;
                        let me = runtime.identity().id().to_string();
                        let msg = PositionMsg { player: me, x, y, seq };
                        if let Ok(bytes) = serde_json::to_vec(&msg) {
                            let _ = runtime.network.publish(&room_topic(&room), bytes).await;
                        }
                    }
                }
            }
        }
    });
}

#[tauri::command]
async fn move_player(tx: State<'_, Arc<mpsc::Sender<(f32,f32)>>>, x: f32, y: f32) -> Result<(), String> {
    tx.send((x, y)).map_err(|e| e.to_string())
}
```

**Verify it works:** move the player and watch the send side emit at most
N frames/second (visible in DevTools as message rate), while the other client
still tracks the position smoothly — receiving fresh values, not replaying
every dropped frame.

## 4. Ordering and staleness — what you must not assume

Gossipsub gives you **no ordering guarantee and no delivery guarantee**:
frames can arrive out of order, and a briefly-disconnected peer simply miss
a window. Three cheap rules keep the game honest:

1. **Sequence per player.** Each player numbers their frames; a client that
   sees a `seq` older than the one it already has for that player drops it —
   that's your replay/duplicate guard.
2. **Never act twice per sequence.** Apply a frame at most once per
   `(player, seq)`.
3. **Render recent only.** If no frame arrived from `p` for a few seconds,
   mark them disconnected (and keep the last position dimmed rather than
   freezing a stale ghost as "live").

```ts
const seen = new Map<string, number>();
function upsertPlayer(msg) {
  if ((seen.get(msg.player) ?? -1) >= msg.seq) return; // stale or duplicate
  seen.set(msg.player, msg.seq);
  render(msg);
}
```

**Verify it works:** feed the canvas frames out of order (e.g. temporarily
reorder in a debug hook) — stale `seq`s are dropped and the game state never
accepts an out-of-order write, so both screens agree on "where everyone
ended up."

## 5. No object storage — and when to add it

Everything so far is ephemeral, and that's a feature: a match's state
genuinely starts fresh each time, and nothing about a match is meant to
outlive it. The moment you want something durable — a high-score table, a
round history, a save file — reuse the object model from the other tutorials
instead of inventing persistence here: `put_object` a JSON snapshot and
share it under a name, exactly like the chat history step in
[tauri-chat-app.md](tauri-chat-app.md#4-history-a-json-snapshot-stored-and-shared-under-a-name).

**Verify it works (end to end):** two clients in one room see each other's
sprites move in near-real-time on the shared topic; disconnect one for a
few seconds and the other's ghost dims, then reconnects and resumes at the
correct `seq` without duplicated frames.