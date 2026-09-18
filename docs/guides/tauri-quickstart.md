# Tutorial: a Tauri app with an embedded runtime — no separate node process

You'll build a minimal desktop app that puts and lists Canopee objects with
**no `canopee-node` process anywhere on the machine**. Instead of spawning a
daemon and talking to it over `node.sock`, the app embeds a
`canopee-runtime::Runtime` directly in its own process via
`Runtime::open_with_config(Config)` and exposes it to a web frontend through
Tauri commands.

This is the foundation every other Tauri tutorial in this series
(chat, collab editor, game, presence) builds on — the "install the app,
that's it" pattern. It exercises: the embedded-runtime model from
[Architecture](../concepts/architecture.md), `Config`'s per-app/user root
split from [Filesystem layout](../reference/filesystem.md), and signed
objects from [Objects & pointers](../concepts/objects.md).

## 1. Scaffold a Tauri app and add the Canopee crates

Create a new Tauri project (`cargo create-tauri-app` or the VS Code Tauri
extension). Add the crates to `Cargo.toml`:

```toml
[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
tokio = { version = "1", features = ["full"] }
canopee-runtime = { path = "../crates/canopee-runtime" }
canopee-config  = { path = "../crates/canopee-config" }
canopee-storage = { path = "../crates/canopee-storage" }
```

(`canopee-storage` is only needed for the `ObjectType` and `Object`
types used by the commands below.)

**Verify it works:** `cargo check` compiles. Nothing Canopee-linked is
running yet.

## 2. Open the Runtime in-process during Tauri's `setup`

The `Runtime` is opened once, at app startup, and held in Tauri managed state
so every `#[tauri::command]` can reach it. Use
`Config::with_app_root(your_app_data_dir)` so **identity and storage stay in
the shared `~/.canopee` user root, while runtime state is per-app** — the
"state is per-app, data is per-user" split. Two instances of this app on one
machine share the user's identity but never collide on sockets or caches.

```rust
use std::sync::Arc;
use tauri::Manager;
use canopee_runtime::Runtime;
use canopee_config::Config;

tauri::Builder::default()
    .setup(|app| {
        // OS-appropriate per-app dir, e.g. ~/Library/Application Support/<app>.
        let app_data = app.path().app_data_dir()?;

        tauri::async_runtime::block_on(async move {
            let config = Config::new()
                .with_app_root(app_data) // runtime state per-app...
                .with_mdns(false);       // ...identity/data stay in ~/.canopee
            let runtime = Arc::new(Runtime::open_with_config(config).await?);
            runtime.mark_started().await?; // enables periodic record sync
            app.manage(runtime);
            Ok::<(), anyhow::Error>(())
        })?;
        Ok(())
    })
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
```

Two notes on the config choice:

- `Config::with_root(dir)` (or `Runtime::open_with_root(dir)`) collapses
  app and user roots into one directory — **full isolation**: a fresh
  identity and store for this app, independent of `~/.canopee`. Use it for
  apps that want their own persona (like the multiplayer-game tutorial).
- `mDNS` is off in app-root mode so several apps sharing one identity don't
  re-announce the same device over multicast (see
  [Networking](../concepts/networking.md)).

**Verify it works:** launch the app. There is no `canopee-node` in your
process list; `ls ~/.canopee/identity/` shows `identity.key` and `device.key`
were created by the app itself (the compiled `canopee-node` binary is never
invoked).

## 3. Commands: `put_text`, `get_object`, `list_objects`

Managed `Arc<Runtime>` is available in commands as
`State<'_, Arc<Runtime>>`. Each command maps a `Runtime` method to a frontend-
callable function and maps `anyhow` errors to `String`.

```rust
use canopee_runtime::Runtime;
use canopee_storage::{ObjectId, ObjectType};
use serde::Serialize;
use std::sync::Arc;
use tauri::State;

#[derive(Serialize)]
struct ObjectSummary {
    id: String,
    name: Option<String>,
    size: u64,
}

#[tauri::command]
async fn put_text(runtime: State<'_, Arc<Runtime>>, text: String) -> Result<String, String> {
    let obj = runtime
        .put_object(text.into_bytes(), ObjectType::Blob, Some("note".into()))
        .await
        .map_err(|e| e.to_string())?;
    Ok(obj.id.to_string())
}

#[tauri::command]
async fn get_object(runtime: State<'_, Arc<Runtime>>, id: String) -> Result<String, String> {
    let object = runtime
        .get(&ObjectId::new(&id))
        .await
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&object.payload.data).into_owned())
}

#[tauri::command]
async fn list_objects(runtime: State<'_, Arc<Runtime>>) -> Result<Vec<ObjectSummary>, String> {
    let objects = runtime.list().await.map_err(|e| e.to_string())?;
    Ok(objects
        .into_iter()
        .map(|o| ObjectSummary {
            id: o.id.to_string(),
            name: o.name,
            size: o.size,
        })
        .collect())
}
```

Every `get` goes through the verified path — the object's hash and signature
are rechecked before anything is returned (see
[Identity](../concepts/identity.md)).

**Verify it works:** with the app running, open DevTools and call
`invoke('list_objects')` from the console — it returns `[]` (or your earlier
objects). No backend service is involved.

## 4. A tiny frontend that puts and lists

Install the Tauri JS API in the frontend (`npm i @tauri-apps/api`) and wire
the commands up:

```ts
import { invoke } from "@tauri-apps/api/core";

async function saveNote(text: string) {
  const id = await invoke<string>("put_text", { text });
  await refresh();
}

async function refresh() {
  const list = await invoke<{ id: string; name: string | null }[]>("list_objects");
  // render list...
}

async function openNote(id: string) {
  const text = await invoke<string>("get_object", { id });
  // show text in a <textarea>...
}
```

**Verify it works:** type a note in the app, save it, and see it appear in the
list after `refresh()`. Save the same note from two app instances on the same
machine and confirm both lists grow — the per-app root keeps them independent.

## 5. Where object sharing meets the network

`put_object` stores **locally and privately** — nothing is shared until you
explicitly say so. When a note should be reachable by other identities, share
it under a name:

```rust
let obj = runtime.put_object(bytes, ObjectType::Blob, Some("note".into())).await?;
runtime.share_object("note", &obj.id, None).await?;
```

Sharing publishes an `(owner, "entry:<note>")` pointer and announces this
node as a provider — both halves of [Sharing](../concepts/objects.md).
The [chat](tauri-chat-app.md) and [collab editor](tauri-collab-editor.md)
tutorials use exactly this pattern for history documents.

**Verify it works:** from a second identity (`canopee-cli`), resolve and
fetch the shared note while your app is running.

## Next steps

- [Chat app](tauri-chat-app.md) — real-time pub/sub on top of this foundation.
- [Collab editor](tauri-collab-editor.md) — shared documents synced via pointers.
- [Presence app](tauri-presence-app.md) — the smallest real app in the set.
- The full embedded/socket client surface is in the [SDK reference](../reference/sdk.md).