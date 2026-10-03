use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_sdk::NodeClient;

pub(crate) async fn init() {
    let runtime = Runtime::open().await.unwrap();
    println!("Canopee initialized:");
    println!("Identity: {:?}", runtime.identity().id());
}

/// Locates the `canopee-node` binary to launch.
///
/// Prefers the node shipped next to this executable — that's the copy the
/// user installed, and it keeps `canopee start` from depending on a source
/// tree or a Rust toolchain. `CANOPEE_NODE_BIN` overrides it, and the
/// `target/{profile}` sibling is the last resort for running from a checkout.
fn node_binary() -> anyhow::Result<std::path::PathBuf> {
    if let Some(explicit) = std::env::var_os("CANOPEE_NODE_BIN") {
        let path = std::path::PathBuf::from(explicit);
        if !path.exists() {
            anyhow::bail!(
                "CANOPEE_NODE_BIN points at {}, which does not exist",
                path.display()
            );
        }
        return Ok(path);
    }

    let exe = std::env::current_exe()?;
    if let Some(dir) = exe.parent() {
        let sibling = dir.join("canopee-node");
        if sibling.exists() {
            return Ok(sibling);
        }
        // Running out of `target/debug` or `target/release` via cargo.
        if let Some(profile) = dir.file_name() {
            let built = dir.parent().map(|t| t.join(profile).join("canopee-node"));
            if let Some(built) = built {
                if built.exists() {
                    return Ok(built);
                }
            }
        }
    }

    anyhow::bail!(
        "could not find the `canopee-node` binary next to {} — build it with \
         `cargo build --release -p canopee-node`, or set CANOPEE_NODE_BIN",
        exe.display()
    )
}

pub(crate) async fn start() {
    let node = match node_binary() {
        Ok(node) => node,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(1);
        }
    };

    // Refuse to start a second daemon on the same app root: the socket is
    // the lock, and letting two daemons fight over one identity/store is
    // exactly the instability this command used to cause.
    if let Ok(existing) = NodeClient::new().await {
        if existing.is_running().await {
            eprintln!("Error: the Canopee node is already running for this app root.");
            std::process::exit(1);
        }
    }

    let child = tokio::process::Command::new(&node)
        .stdin(std::process::Stdio::null())
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Error: could not start {}: {e}", node.display());
            std::process::exit(1);
        }
    };

    let pid = child.id().unwrap_or(0);
    // Detach: the daemon must outlive this CLI invocation, and its stdio
    // must not hold the terminal open.
    drop(child);

    println!("Canopee node started (pid {pid})");
    println!("Node binary: {}", node.display());
}

pub(crate) async fn status() {
    let client = super::common::node_or_exit().await;
    let response = client.request(NodeCommand::Status).await.unwrap();
    match response {
        NodeResponse::Status {
            identity,
            objects,
            peers,
        } => {
            println!("\nCanopee Node");
            println!("\nRunning:");
            println!("yes"); // placeholder
            println!("\nIdentity:");
            println!("{}", identity);
            println!("\nObjects:");
            println!("{}", objects);
            println!("\nPeers:");
            println!("{}", peers);
        }
        NodeResponse::Error { message } => {
            eprintln!("Error: {}", message);
        }
        _ => {}
    }
}

pub(crate) async fn stop() {
    let client = super::common::node_or_exit().await;
    let response = client.request(NodeCommand::Shutdown).await.unwrap();

    match response {
        NodeResponse::ShutdownAccepted => {
            println!("Canopee node stopped");
        }

        NodeResponse::Error { message } => {
            eprintln!("{}", message);
        }

        _ => {}
    }
}
