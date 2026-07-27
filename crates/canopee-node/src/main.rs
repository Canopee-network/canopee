use canopee_node::Node;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let node = Node::open().await?;

    println!("Canopee node started");

    node.run().await?;

    Ok(())
}
