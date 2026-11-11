use super::common::client_or_exit;
use canopee_sdk::SyncRootInfo;

pub(crate) async fn create(path: String, name: Option<String>) {
    let client = client_or_exit().await;
    match client.sync_create_root(path, name).await {
        Ok(info) => print_root("Created", &info),
        Err(e) => fail(e),
    }
}

pub(crate) async fn join(name: String, path: String) {
    let client = client_or_exit().await;
    match client.sync_join_root(name, path).await {
        Ok(info) => print_root("Joined", &info),
        Err(e) => fail(e),
    }
}

pub(crate) async fn list() {
    let client = client_or_exit().await;
    match client.sync_list_roots().await {
        Ok(roots) => print_roots(&roots),
        Err(e) => fail(e),
    }
}

pub(crate) async fn status(name: Option<String>) {
    let client = client_or_exit().await;
    match client.sync_status(name).await {
        Ok(roots) => print_roots(&roots),
        Err(e) => fail(e),
    }
}

pub(crate) async fn now(name: Option<String>) {
    let client = client_or_exit().await;
    match client.sync_now(name).await {
        Ok(results) if results.is_empty() => println!("No local sync roots"),
        Ok(results) => {
            for result in results {
                if result.changed() {
                    println!(
                        "{}: {} uploaded, {} downloaded, {} removed, {} conflicts",
                        result.root,
                        result.uploaded,
                        result.downloaded,
                        result.removed,
                        result.conflicts
                    );
                } else {
                    println!("{}: already up to date", result.root);
                }
            }
        }
        Err(e) => fail(e),
    }
}

pub(crate) async fn remove(name: String) {
    let client = client_or_exit().await;
    match client.sync_remove_root(name.clone()).await {
        Ok(()) => println!("Removed local mapping for \"{name}\""),
        Err(e) => fail(e),
    }
}

fn print_root(verb: &str, info: &SyncRootInfo) {
    println!(
        "{verb} sync root \"{}\" at {} (revision {})",
        info.name,
        info.path
            .as_deref()
            .unwrap_or("(not joined on this device)"),
        info.revision
    );
}

fn print_roots(roots: &[SyncRootInfo]) {
    if roots.is_empty() {
        println!("No sync roots");
        return;
    }
    for root in roots {
        println!(
            "{}  {}  revision {}{}",
            root.name,
            root.path.as_deref().unwrap_or("(not joined)"),
            root.revision,
            root.last_sync_at
                .map(|t| format!("  last sync {t}"))
                .unwrap_or_default()
        );
    }
}

fn fail(error: anyhow::Error) -> ! {
    eprintln!("Error: {error}");
    std::process::exit(1);
}
