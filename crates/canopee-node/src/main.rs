use canopee_node::Node;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize a tracing subscriber when RUST_LOG is set (e.g.
    // `RUST_LOG=libp2p_swarm=debug canopee-node`), so network internals are
    // observable without a compile-time flag. A bare `canopee-node` run stays
    // quiet, matching the original minimal-output behavior.
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .init();
    }
    let node = Node::open().await?;
    println!("Canopee node started");
    node.run().await?;
    println!("Canopee node exited");

    Ok(())
}
