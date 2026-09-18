pub(crate) async fn gateway(port: Option<u16>) {
    let token = canopee_gateway::SessionToken::new();
    let gateway = match canopee_gateway::Gateway::start_on(token.clone(), port).await {
        Ok(gateway) => gateway,
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    };

    println!("Canopee gateway ready for this machine only:");
    println!("  Demo page: {}", gateway.page_url());
    println!("  WebSocket: {}", gateway.session_url(&token));
    println!();
    println!(
        "Open the demo page in a browser, or point your app's JavaScript at the \
         WebSocket URL with the client from crates/canopee-gateway/www/client.js."
    );
    println!("Press Ctrl-C to stop.");

    tokio::select! {
        result = gateway.serve() => {
            if let Err(e) = result {
                eprintln!("Gateway error: {e:#}");
                std::process::exit(1);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("Stopping gateway");
        }
    }
}