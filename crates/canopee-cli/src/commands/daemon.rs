use canopee_protocol::{NodeCommand, NodeResponse};
use canopee_runtime::Runtime;
use canopee_sdk::NodeClient;

pub(crate) async fn init() {
    let runtime = Runtime::open().await.unwrap();
    println!("Canopee initialized:");
    println!("Identity: {:?}", runtime.identity().id());
}

pub(crate) async fn start() {
    let child = tokio::process::Command::new("cargo")
        .args(["run", "-p", "canopee-node"])
        .spawn()
        .unwrap();

    println!("Canopee node started (pid {})", child.id().unwrap());
}

pub(crate) async fn status() {
    let client = NodeClient::new().await.unwrap();
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
    let client = NodeClient::new().await.unwrap();
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
