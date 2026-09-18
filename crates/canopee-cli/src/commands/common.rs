use crate::app::{fetch_app, serve};
use canopee_sdk::CanopeeClient;
use canopee_storage::{ExportBundle, ObjectId};
use std::path::Path;

/// Imports a fetched bundle, treating "already stored" as success: fetching
/// an object you already hold is a no-op, not an error. Any other failure
/// exits so the fetch command never silently claims success.
pub(crate) async fn import_idempotent(client: &CanopeeClient, bundle: ExportBundle) {
    match client.import(bundle).await {
        Ok(()) => {}
        Err(e) if e.to_string().contains("already exists") => {}
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    }
}

/// Writes a stored object's bytes to a temp file (keeping the name — and
/// with it the file extension, so the OS picks the right handler) and opens
/// it with the system's default app. The temp copy is left in place so the
/// viewer can keep reading it; the path is printed for reference.
pub(crate) async fn open_with_default_app(data: &[u8], name: &str) {
    let dir = std::env::temp_dir().join("canopee-open");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Error: cannot create temp dir {}: {e}", dir.display());
        std::process::exit(1);
    }
    // Never let a stored name escape the temp dir.
    let file_name = Path::new(name)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "object".to_string());
    let path = dir.join(file_name);
    if let Err(e) = tokio::fs::write(&path, data).await {
        eprintln!("Error: cannot write {}: {e}", path.display());
        std::process::exit(1);
    }
    println!("Opening {}", path.display());
    #[cfg(target_os = "macos")]
    {
        match std::process::Command::new("open").arg(&path).spawn() {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error: could not launch `open`: {e}");
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Opening files with the system app is only wired up on macOS yet.");
        eprintln!("The file is at: {}", path.display());
    }
}

/// Fetches an app manifest (and its assets, from a peer or the DHT when not
/// stored locally) and serves it over HTTP. The app half of `open`.
pub(crate) async fn serve_app_on_http(
    client: &CanopeeClient,
    manifest_id: ObjectId,
    peer: Option<String>,
    port: u16,
    open: bool,
) {
    let (manifest, files) = match fetch_app(client, manifest_id, peer).await {
        Ok(app) => app,
        Err(e) => {
            eprintln!("Error: failed to open app: {e:#}");
            std::process::exit(1);
        }
    };
    println!("Opening \"{}\" by {}", manifest.name, manifest.owner);
    serve(files, port, open).await.unwrap();
}

/// Resolves an object's recorded name from the local listing (names live in
/// the storage sidecar, not inside the signed object).
pub(crate) async fn object_name(client: &CanopeeClient, id: &str) -> Option<String> {
    client
        .list()
        .await
        .ok()?
        .into_iter()
        .find(|o| o.id.0 == id)
        .and_then(|o| o.name)
}

/// True when `s` looks like a content-addressed object id: exactly 64 hex
/// characters. Anything else is treated as a human name.
pub(crate) fn looks_like_id(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolves a CLI object argument to an id plus a display name. Accepts a
/// 64-char hex id (passed through), or a human name known locally: either a
/// storage sidecar name (from `put <file>`) or a home-index entry name (from
/// `share`). Persistent ids are only meant for machine-to-machine use — this
/// CLI is designed around names.
pub(crate) async fn resolve_object_arg(
    client: &CanopeeClient,
    arg: &str,
) -> anyhow::Result<(ObjectId, String)> {
    if looks_like_id(arg) {
        return Ok((ObjectId::new(arg), arg.to_string()));
    }
    let mut matches: Vec<ObjectId> = Vec::new();
    for object in client.list().await? {
        if object.name.as_deref() == Some(arg) {
            matches.push(object.id);
        }
    }
    // Only fall back to home-index entries when the sidecar listing found
    // nothing. Resolving the home index can touch the DHT (unbounded
    // round-trips when the `(owner, "home")` record isn't cached locally) —
    // never make a name user already has on disk wait on the network.
    if matches.is_empty() {
        if let Some(index) = client.load_home_index().await? {
            for entry in index.entries {
                if entry.name == arg {
                    matches.push(entry.object);
                }
            }
        }
    }
    match matches.len() {
        0 => anyhow::bail!(
            "no local object named \"{arg}\" (put it first with `canopee put`, \
             then try again — or pass the 64-char id)"
        ),
        1 => Ok((matches.pop().unwrap(), arg.to_string())),
        n => anyhow::bail!(
            "ambiguous: {n} local objects are named \"{arg}\" — pass the 64-char id to disambiguate"
        ),
    }
}

/// Shortens a raw libp2p peer id for display (`12D3KooW…abcd`) so terminal
/// output isn't dominated by a 39-character base58 blob.
pub(crate) fn short_peer_id(peer_id: &str) -> String {
    const HEAD: usize = 12;
    const TAIL: usize = 4;
    if peer_id.len() <= HEAD + TAIL + 1 {
        return peer_id.to_string();
    }
    format!("{}…{}", &peer_id[..HEAD], &peer_id[peer_id.len() - TAIL..])
}

/// Hex of the first few bytes of a binary blob, for previews ("68 65 6c").
pub(crate) fn hex_preview(data: &[u8]) -> String {
    data.iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Resolves a CLI peer argument to a dialable peer id string. Accepts:
/// - a raw libp2p peer id (passed through),
/// - a canonical identity `canopee://identity/<peer-id>` (resolved to one of
///   the owner's registered *device* peer ids — since device-key separation,
///   the identity-bound peer id is not a live network address),
/// - a friendly username, reverse-resolved via the DHT registry to its owner,
///   then to one of the owner's devices.
///
/// Falls back to the legacy identity-bound peer id when the owner has no
/// registered device (e.g. pre-device-key peers), keeping the old dial path
/// working for already-paired peers.
pub(crate) async fn resolve_peer_arg(client: &CanopeeClient, arg: &str) -> anyhow::Result<String> {
    Ok(resolve_owner_arg(client, arg).await?.1)
}

/// Like [`resolve_peer_arg`], but also returns the resolved owner identity —
/// needed by name-based fetch, which resolves the owner's `(owner, "entry:
/// <name>")` pointer before dialling one of their devices.
pub(crate) async fn resolve_owner_arg(
    client: &CanopeeClient,
    arg: &str,
) -> anyhow::Result<(canopee_sdk::IdentityId, String)> {
    let arg = arg.trim();
    // Raw peer id: libp2p Ed25519 peer ids always start with this prefix.
    if arg.starts_with("12D3KooW") {
        // The identity is unknown; derive the identity-scoped owner and dial
        // the peer directly (legacy identity-bound path).
        let owner = canopee_sdk::IdentityId::new(format!("canopee://identity/{arg}"));
        return Ok((owner, arg.to_string()));
    }
    // Canonical identity form: the embedded peer id doubles as the owner.
    let owner = if let Some(peer_id) = arg.strip_prefix("canopee://identity/") {
        Some(canopee_sdk::IdentityId::new(format!("canopee://identity/{peer_id}")))
    } else {
        // Otherwise treat it as a username and reverse-resolve it to its owner.
        client.resolve_username(arg).await?
    };
    let Some(owner) = owner else {
        anyhow::bail!(
            "\"{arg}\" is not a valid peer id, and no username \"{arg}\" is claimed \
             (check with `canopee username lookup {arg}`)"
        );
    };
    // Prefer the owner's registered device (device-key phase); fall back to
    // the identity-bound peer id for legacy peers that never registered.
    if let Some(device) = client.resolve_owner_device(&owner).await? {
        return Ok((owner, device));
    }
    let peer_id = owner
        .to_string()
        .strip_prefix("canopee://identity/")
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("cannot resolve \"{arg}\" to a dialable peer"))?;
    Ok((owner, peer_id))
}