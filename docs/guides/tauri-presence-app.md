# Tutorial: a presence app — who's online right now

You'll build the smallest real Canopee app: a window listing the people
currently around — by their profile display name, not a raw peer id. It
combines two signals: a periodic poll of connected peers (enriched through
the profile/identity records) and a lightweight "I'm here" heartbeat you
publish to a shared topic so others know you're online even before a direct
connection matters.

Foundation: [tauri-quickstart.md](tauri-quickstart.md) (managed
`Arc<Runtime>`). Concepts: [Identity](../concepts/identity.md) for the
device-vs-person split and profiles, [Networking](../concepts/networking.md)
for pub/sub and peer discovery.

## 1. Give yourself a display name via `save_profile`

Presence is about people, so establish a persona first: `save_profile` stores
a signed `Profile` under your `(owner, "profile")` record — the display name
anyone else can resolve:

```rust
#[tauri::command]
async fn set_display_name(
    runtime: State<'_, Arc<Runtime>>,
    name: String,
) -> Result<(), String> {
    let profile = canopee_storage::Profile {
        display_name: name,
        dh_public_key: runtime.identity().dh_public_key(),
        avatar: None,
        version: 0,
    };
    runtime.save_profile(&profile).await.map_err(|e| e.to_string())
}
```

Because it's a signed record behind a pointer, your name travels with your
identity — any peer who can resolve `(you, "profile")` sees the same string
(see [Objects & pointers](../concepts/objects.md)).

**Verify it works:** call `set_display_name("alice")`, then `load_profile`
returns `alice` back — even after restarting the app.

## 2. Broadcast a heartbeat on a presence topic

Presence needs a "still here" signal others can count. Subscribe to a shared
topic and publish a small heartbeat every few seconds, tagged with your
identity and display name:

```rust
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct PresencePong {
    identity: String,
    display_name: String,
    ts: u64, // unix seconds
}

fn presence_topic(room: &str) -> String {
    format!("presence-{room}")
}

fn spawn_heartbeat(app: tauri::AppHandle, runtime: Arc<Runtime>, room: String) {
    tauri::async_runtime::spawn(async move {
        let mut rx = match runtime.network.subscribe(&presence_topic(&room)).await {
            Ok(rx) => rx,
            Err(e) => { eprintln!("subscribe failed: {e}"); return; }
        };
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tokio::select! {
                Ok(frame) = rx.recv() => {
                    // Someone else's heartbeat — forward to the frontend.
                    if let Ok(pong) = serde_json::from_slice::<PresencePong>(&frame.data) {
                        let _ = app.emit("presence-pong", &pong);
                    }
                }
                _ = ticker.tick() => {
                    let identity = runtime.identity().id().to_string();
                    let display_name = runtime
                        .load_profile()
                        .await
                        .ok()
                        .flatten()
                        .map(|p| p.display_name)
                        .unwrap_or_else(|| identity.clone());
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let pong = PresencePong { identity, display_name, ts };
                    if let Ok(bytes) = serde_json::to_vec(&pong) {
                        let _ = runtime.network.publish(&presence_topic(&room), bytes).await;
                    }
                }
            }
        }
    });
}
```

Ping every 3–5 seconds (matching gossipsub's own 1s heartbeat granularity)
and treat a peer as *offline* once you haven't heard from them for ~2–3
intervals — one missed heartbeat shouldn't flicker the UI.

**Verify it works:** two instances in the same room; each receives the
other's heartbeat on the topic within a few seconds of startup.

## 3. Poll connected peers and enrich them

Heartbeats say "someone on the topic is around"; `peers()` says "I am
directly connected to this device". To present *people*, map connected peers
to identities and enrich them with display names, using the same machinery
the CLI uses:

```rust
#[tauri::command]
async fn presence_list(runtime: State<'_, Arc<Runtime>>) -> Result<Vec<PeerSummary>, String> {
    // Resolve each connected peer's identity + username/profile display name,
    // pushing results back into the network manager's peer map.
    runtime.enrich_peers().await.map_err(|e| e.to_string())?;

    let peers = runtime.network.peers().await.map_err(|e| e.to_string())?;
    Ok(peers
        .into_iter()
        .map(|p| PeerSummary {
            identity: p.identity.map(|id| id.to_string()).unwrap_or_default(),
            display_name: p.display_name.clone().or(p.username.clone()).unwrap_or_else(|| p.peer_id.to_string()),
        })
        .collect())
}
```

`enrich_peers` resolves each connected peer's device → identity via the
device registry and then their signed profile/username record — a peer's
libp2p `PeerId` is the *device*, never the person (see
[Identity](../concepts/identity.md)), so you never format the peer id
into a name yourself.

**Verify it works:** with a second instance connected, `presence_list`
shows its profile display name (not a raw `12D3Koo...` peer id).

## 4. Merge the two signals in the frontend

The frontend keeps a map keyed by identity: connected peers (from the poll)
plus anyone heard on the topic recently (from the heartbeat listener),
each with a `last_seen`. A periodic refresh recomputes online/offline:

```ts
interface PresentPeer { identity: string; displayName: string; lastSeen: number; }

let present = new Map<string, PresentPeer>();

async function refreshPresence() {
  const connected = await invoke<{ identity: string; display_name: string }[]>("presence_list");
  for (const p of connected) {
    present.set(p.identity, {
      identity: p.identity,
      displayName: p.display_name,
      lastSeen: Date.now(),
    });
  }
  const now = Date.now();
  for (const [, p] of present) {
    p.dead = now - p.lastSeen > 15_000; // ~3 heartbeat intervals offline
  }
  renderPresence([...present.values()]); // name + online/offline dot
}

listen<PresentPeer>("presence-pong", (e) => present.set(e.payload.identity, {
  ...e.payload, lastSeen: Date.now(),
}));
setInterval(refreshPresence, 5000);
```

The two signals cover each other's blind spots: heartbeats keep a contact
"online" even between direct-connection flaps, and `peers()` keeps a
directly-connected device visible even if its heartbeat is briefly delayed.

**Verify it works:** launch a second instance; within ~5s its display name
appears on the first's presence list with an online dot. Quit it — the dot
goes grey after the 15s staleness window, even without any explicit
"goodbye" message, because a crashed app can't send one.

## 5. Making list entries actionable

Presence is a primitive, not a feature — wire it onward:

- Resolve the peer's device to dial them (`resolve_device_peer_id` / the
  device list) and add a "send chat" button that hands the room topic off to
  the [chat app](tauri-chat-app.md).
- Persist your own display name in local app state so you don't need to
  re-enter it, then `save_profile` once at startup.
- For a contact list beyond raw peers, the `(owner, "contacts")` record
  (`save_contact_list` / `load_contact_list`) is the natural owner; see the
  [SDK reference](../reference/sdk.md).

**Verify it works (end to end):** two instances each show the other by name,
flip to offline on quit, and (after a restart) still show the stored display
name before any heartbeat has been exchanged.