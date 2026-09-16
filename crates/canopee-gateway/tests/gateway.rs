//! End-to-end test of the gateway: a real in-process [`Node`] behind a real
//! [`Gateway`], driven over the wire by a hand-rolled WebSocket *client* (the
//! same exchange a browser's `WebSocket` object performs).

use canopee_gateway::{Gateway, SessionToken};
use canopee_node::Node;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const OP_TEXT: u8 = 0x1;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xa;
const OP_CLOSE: u8 = 0x8;

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Performs an RFC 6455 client handshake against the gateway. Returns the
/// stream on `101`, or `None` when the gateway refuses (403).
async fn handshake(
    addr: std::net::SocketAddr,
    token: Option<&str>,
    origin: Option<&str>,
) -> anyhow::Result<Option<TcpStream>> {
    let mut stream = TcpStream::connect(addr).await?;
    // A fixed but valid base64 16-byte key; the server doesn't care how it
    // was generated, only that it hashes to the RFC answer.
    let key = "dGhlIHNhbXBsZSBub25jZQ==";
    let mut request = format!(
        "GET /?token={} HTTP/1.1\r\n\
         Host: 127.0.0.1\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n",
        token.unwrap_or("")
    );
    if let Some(origin) = origin {
        request.push_str(&format!("Origin: {origin}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            break;
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let status = String::from_utf8_lossy(&head);
    let is_101 = status.lines().next().is_some_and(|l| l.contains(" 101 "));
    Ok(if is_101 { Some(stream) } else { None })
}

/// Sends one masked (client→server) frame.
async fn send_frame(stream: &mut TcpStream, opcode: u8, payload: &[u8]) -> anyhow::Result<()> {
    let mut header = Vec::new();
    header.push(0x80 | opcode);
    let length = payload.len();
    if length < 126 {
        header.push(0x80 | length as u8);
    } else if length <= u16::MAX as usize {
        header.push(0x80 | 126);
        header.extend_from_slice(&(length as u16).to_be_bytes());
    } else {
        header.push(0x80 | 127);
        header.extend_from_slice(&(length as u64).to_be_bytes());
    }
    let mask = [0x11, 0x22, 0x33, 0x44];
    header.extend_from_slice(&mask);
    let mut masked = payload.to_vec();
    for (i, byte) in masked.iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
    stream.write_all(&header).await?;
    stream.write_all(&masked).await?;
    stream.flush().await?;
    Ok(())
}

/// Reads one (unmasked, server→client) frame.
async fn read_frame(stream: &mut TcpStream) -> anyhow::Result<(u8, Vec<u8>)> {
    let mut b0 = [0u8; 1];
    stream.read_exact(&mut b0).await?;
    let mut b1 = [0u8; 1];
    stream.read_exact(&mut b1).await?;
    let opcode = b0[0] & 0x0f;
    let mut length = (b1[0] & 0x7f) as u64;
    if length == 126 {
        let mut bytes = [0u8; 2];
        stream.read_exact(&mut bytes).await?;
        length = u16::from_be_bytes(bytes) as u64;
    } else if length == 127 {
        let mut bytes = [0u8; 8];
        stream.read_exact(&mut bytes).await?;
        length = u64::from_be_bytes(bytes);
    }
    let mut payload = vec![0u8; length as usize];
    stream.read_exact(&mut payload).await?;
    Ok((opcode, payload))
}

/// Sends a command and reads the JSON event the gateway replies with.
async fn command(
    stream: &mut TcpStream,
    body: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let bytes = serde_json::to_vec(body)?;
    send_frame(stream, OP_TEXT, &bytes).await?;
    loop {
        let (opcode, payload) = read_frame(stream).await?;
        match opcode {
            OP_TEXT => return Ok(serde_json::from_slice(&payload)?),
            OP_PONG => continue,
            OP_CLOSE => anyhow::bail!("gateway closed the connection"),
            _ => anyhow::bail!("unexpected frame opcode {opcode:#x}"),
        }
    }
}

fn json_op(op: &str) -> serde_json::Value {
    serde_json::json!({ "op": op })
}

/// Boots one node on scratch `$HOME`, starts a gateway in front of it, and
/// exercises the whole stack: demo page, token/origin enforcement, protocol
/// commands, and the streaming subscription path. Kept to a single test (with
/// one `$HOME` + one node) because `HOME` is process-global.
#[tokio::test(flavor = "multi_thread")]
async fn browser_drives_node_through_gateway() {
    let home = std::env::temp_dir().join(format!("canopee_gateway_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // SAFETY: this test binary runs this single test; no other thread reads HOME concurrently.
    unsafe { std::env::set_var("HOME", &home) };

    let node = Node::open().await.unwrap();
    let node_for_task = node.clone();
    tokio::spawn(async move {
        let _ = node_for_task.run().await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let token = SessionToken::new();
    let gateway = Gateway::start(token.clone()).await.unwrap();
    let addr = gateway.addr();
    let serve_task = tokio::spawn(async move {
        let _ = gateway.serve().await;
    });

    // ---- demo page: GET / is served, nothing else is ----
    {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let mut body = Vec::new();
        stream.read_to_end(&mut body).await.unwrap();
        let page = String::from_utf8_lossy(&body);
        assert!(
            page.starts_with("HTTP/1.1 200 OK"),
            "demo page must be served on GET /: {page}"
        );
        assert!(page.contains("canopee gateway demo"));
    }

    // ---- auth: wrong/missing tokens and foreign origins are refused ----
    assert!(
        handshake(addr, None, None).await.unwrap().is_none(),
        "no token"
    );
    assert!(
        handshake(addr, Some("wrong-token"), None)
            .await
            .unwrap()
            .is_none(),
        "bad token"
    );
    assert!(
        handshake(addr, Some(&token.token_hex()), Some("https://evil.example"))
            .await
            .unwrap()
            .is_none(),
        "cross-origin handshake"
    );
    assert!(
        handshake(addr, Some(&token.token_hex()), Some("http://127.0.0.1:1"))
            .await
            .unwrap()
            .is_some(),
        "loopback-origin handshake"
    );

    // ---- established session: identity, storage, and pings ----
    let mut ws = handshake(addr, Some(&token.token_hex()), None)
        .await
        .unwrap()
        .expect("correct token handshake must succeed");

    // Ping/originating keep-alive is answered with a pong.
    send_frame(&mut ws, OP_PING, b"hi").await.unwrap();
    let (opcode, payload) = read_frame(&mut ws).await.unwrap();
    assert_eq!(opcode, OP_PONG);
    assert_eq!(payload, b"hi");

    let event = command(&mut ws, &json_op("identity")).await.unwrap();
    assert_eq!(event["op"], "ok");
    let identity = event["result"]["identity"].as_str().unwrap().to_string();
    assert!(identity.starts_with("canopee://identity/"));

    let event = command(
        &mut ws,
        &serde_json::json!({ "op": "put", "text": "hello browser" }),
    )
    .await
    .unwrap();
    let id = event["result"]["id"].as_str().unwrap().to_string();

    let event = command(&mut ws, &serde_json::json!({ "op": "get", "id": id }))
        .await
        .unwrap();
    assert_eq!(event["op"], "ok");
    assert_eq!(event["result"]["object"]["text"], "hello browser");

    let event = command(&mut ws, &json_op("list")).await.unwrap();
    assert_eq!(event["result"]["objects"].as_array().unwrap().len(), 1);

    // The node has a peerless network: publishing to nobody fails cleanly and
    // flows back over the gateway as an error event.
    let event = command(
        &mut ws,
        &serde_json::json!({ "op": "publish", "topic": "t", "text": "x" }),
    )
    .await
    .unwrap();
    assert_eq!(event["op"], "error");
    assert!(event["message"].as_str().unwrap().len() > 0);

    // ---- streaming subscription path: the connection stays responsive
    // while a subscription pump is running ----
    send_frame(
        &mut ws,
        OP_TEXT,
        b"{\"op\":\"subscribe\",\"topic\":\"gateway-test\"}",
    )
    .await
    .unwrap();
    // Give the gateway time to open its own node client for the pump.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let event = command(&mut ws, &json_op("identity")).await.unwrap();
    assert_eq!(event["op"], "ok");

    // Graceful close: we send a close frame and the gateway closes too.
    send_frame(&mut ws, OP_CLOSE, &[0x03, 0xE8]).await.unwrap();
    let (opcode, _) = read_frame(&mut ws).await.unwrap();
    assert_eq!(opcode, OP_CLOSE);

    let _ = canopee_sdk::CanopeeClient::connect()
        .await
        .unwrap()
        .shutdown()
        .await;
    serve_task.abort();
}
