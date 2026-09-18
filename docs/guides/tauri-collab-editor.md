# Tutorial: a collaborative document editor

You'll build a shared text document that several identities can edit at once,
live, with no server: each edit is stored as a new signed object, a pointer
`(owner, "<doc-name>")` is repointed at it, and a lightweight pub/sub message
tells collaborators "there's a newer version" so they resolve the pointer,
fetch, and verify. Conflict resolution is **last-writer-wins by pointer
timestamp** — the newest signed record wins, which is correct for a small
trusted group (for concurrent simultaneous edits you'd want a CRDT; this
tutorial deliberately keeps the model simple).

Foundation: [tauri-quickstart.md](tauri-quickstart.md) (managed
`Arc<Runtime>`, same dependencies). Concepts: [Objects & pointers](../concepts/objects.md)
for the object+pointer model, [Identity](../concepts/identity.md) for
owner signatures.

## 1. Data model: the document is an object behind a pointer

A document is a signed `Object` holding JSON `{ text, updated_by }`. Its
stable identity across edits is the pointer `(owner, "<name>")` —
`publish_pointer` / `resolve_pointer` — so collaborators always resolve the
*latest* version under that owner+name, and every object they fetch is
hash- and signature-verified before use.

```rust
use serde::Serialize;

#[derive(Serialize)]
struct DocVersion {
    text: String,
    updated_by: String,
}

fn doc_key(name: &str) -> String {
    format!("doc:{name}")
}
```

**Verify it works:** two collaborators agree on a doc name (`"notes"`) and
each owner's identity — that `(owner, name)` pair is the doc's address.

## 2. `save_doc` — put a new version and repoint the pointer

Every time the text changes (debounced, so you're not writing objects per
keystroke), store a new version object and repoint the pointer at it:

```rust
#[tauri::command]
async fn save_doc(
    runtime: State<'_, Arc<Runtime>>,
    name: String,
    text: String,
) -> Result<(), String> {
    let version = DocVersion {
        text,
        updated_by: runtime.identity().id().to_string(),
    };
    let bytes = serde_json::to_vec(&version).map_err(|e| e.to_string())?;
    let obj = runtime
        .put_object(bytes, ObjectType::Blob, None)
        .await
        .map_err(|e| e.to_string())?;
    runtime
        .publish_pointer(&doc_key(&name), obj.id.clone())
        .await
        .map_err(|e| e.to_string())
}
```

`publish_pointer` signs a fresh `(owner, doc-key) → object-id` record with
its own timestamp; both a local cache write and a DHT put happen, so a peer
on the same machine sees the new pointer instantly and a remote peer can
resolve it over the network.

**Verify it works:** call `save_doc` twice with different text; each call
creates a distinct object id and the pointer targets the latest one.

## 3. `load_doc` — resolve the pointer, then get the object

```rust
#[tauri::command]
async fn load_doc(
    runtime: State<'_, Arc<Runtime>>,
    name: String,
) -> Result<String, String> {
    let owner = runtime.identity().id().clone();
    let Some(record) = runtime
        .resolve_pointer(&owner, &doc_key(&name))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(String::new()); // never saved yet
    };
    let object = runtime.get(&record.manifest).await.map_err(|e| e.to_string())?;
    let version: DocVersion =
        serde_json::from_slice(&object.payload.data).map_err(|e| e.to_string())?;
    Ok(version.text)
}
```

`resolve_pointer` checks the local record cache first, then the DHT, and
only accepts records that verify against `owner`; `get` re-verifies the
object. A stale or forged answer simply fails.

**Verify it works:** delete the local doc text, restart the app, call
`load_doc` — the text comes back from the stored object.

## 4. Live sync: notify, resolve, fetch, verify — last writer wins

Live delivery uses gossipsub on a per-doc topic. When an edit lands
(`save_doc`), also publish a tiny notification; a background task on every
client subscribes, and on each notification resolves the *newest* pointer it
can see, compares record timestamps, and applies the newer text.

```rust
fn doc_live_topic(name: &str) -> String {
    format!("doc-live:{name}")
}

fn spawn_doc_listener(app: tauri::AppHandle, runtime: Arc<Runtime>, name: String) {
    tauri::async_runtime::spawn(async move {
        let mut rx = match runtime.network.subscribe(&doc_live_topic(&name)).await {
            Ok(rx) => rx,
            Err(e) => { eprintln!("subscribe failed: {e}"); return; }
        };
        let mut last_seen = 0; // newest published_at we've applied
        while let Ok(_frame) = rx.recv().await {
            let owner = runtime.identity().id().clone();
            let Some(record) = runtime.resolve_pointer(&owner, &doc_key(&name)).await.ok().flatten()
            else {
                continue;
            };
            if record.published_at <= last_seen {
                continue; // older or same record — already applied
            }
            let Ok(object) = runtime.get(&record.manifest).await else { continue };
            let Ok(version) = serde_json::from_slice::<DocVersion>(&object.payload.data) else {
                continue;
            };
            last_seen = record.published_at;
            let _ = app.emit("doc-update", version.text.clone());
        }
    });
}
```

Two properties to notice:

- The pub/sub frame only answers *"has there been a change?"* — it carries no
  text. Content always flows through the verified object path, so a forged
  or corrupt topic message can never inject unverified document text.
- Ordering is decided by the signed pointer's `published_at`, not by network
  arrival order — the classic last-writer-wins rule, same one `sync` uses
  for user records. If two writers race, the newer record wins by
  construction.

**Verify it works:** open the doc on two connected instances; the instance
that just typed sees `"Synced"`, and the other instance's text updates after
it resolves the fresh pointer — you can watch the `"Synced → Stale →
Synced"` indicator flip as edits arrive.

## 5. A small frontend

```ts
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

let synced = true;
const docName = "notes";

onInput(async () => {
  synced = false;
  await invoke("save_doc", { name: docName, text });
  synced = true;
});

listen<string>("doc-update", (e) => {
  if (!focused) textarea.value = e.payload; // don't clobber the active cursor
});
```

Debounce `invoke("save_doc", ...)` in your framework of choice (once per
second is plenty), and skip auto-applying remote updates while the user has
focus so you never fight their cursor (see the CRDT note in the intro for
what production editors do).

**Verify it works:** edit on instance A; within a moment instance B shows
the same text and its indicator returns to `Synced` after the merge.

## Notes and limits

- Everything is keyed on one owner's `(owner, "<name>")` pointer — whoever
  saves first is the doc's "writer" on this machine. A later enhancement is
  to also poll `resolve_pointer` with another collaborator's owner id to
  merge their writes.
- Gossipsub has no delivery guarantee, and this tutorial accepts that: a
  collaborator who misses a live notification still catches up by
  re-resolving the pointer (feed them "refresh" or make the listener
  re-resolve on a timer).
- There is no access control: anyone who knows the doc name can subscribe
  and publish edits. The [SDK reference](../reference/sdk.md) covers the
  capability grants that would gate this properly.