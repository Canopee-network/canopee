use canopee_sdk::CanopeeClient;
use canopee_storage::{AppManifest, Object, ObjectId};
use futures::future::try_join_all;
use std::{collections::HashMap, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

// pub async fn publish_directory(
//     client: &CanopeeClient,
//     directory: &Path,
// ) -> anyhow::Result<(ObjectId, HashMap<String, ObjectId>)> {
//     let mut assets = HashMap::new();
//     let mut entrypoint = None;

//     for entry in walkdir::WalkDir::new(directory)
//         .into_iter()
//         .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
//     {
//         let entry = entry?;
//         if entry.file_type().is_dir() {
//             continue;
//         }
//         let path = entry.path();
//         let data = tokio::fs::read(path).await.unwrap();
//         let object_id = client.put_file(data).await?;
//         let relative = path.strip_prefix(directory)?.to_string_lossy().to_string();

//         println!("{} -> {}", relative, object_id);

//         if relative == "index.html" {
//             entrypoint = Some(object_id.clone());
//         } else {
//             assets.insert(format!("/{}", relative), object_id);
//         }
//     }
//     let entrypoint = entrypoint.ok_or(anyhow::anyhow!("portfolio has no index.html"))?;
//     Ok((entrypoint, assets))
// }

/// Resolves a peer to fetch `id` from: `peer` if given, otherwise the first
/// provider found on the DHT for `id`.
async fn resolve_peer(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(peer) = peer {
        return Ok(peer.to_string());
    }
    let providers = client.find_providers(id.clone()).await?;
    providers
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no providers found for {id}; try passing --peer"))
}

/// Reads an object from local storage, falling back to fetching it from
/// `peer` (discovering a provider via the DHT if none was given) and
/// importing it locally.
async fn get_or_fetch(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<Object> {
    if let Ok(object) = client.get(id.clone()).await {
        return Ok(object);
    }

    let peer_id = resolve_peer(client, id, peer).await?;
    let bundle = client.fetch_object(peer_id, id.clone()).await?;
    client.import(bundle.clone()).await?;
    Ok(bundle.object)
}

/// Resolves an app manifest and every object it references (entrypoint +
/// assets), fetching whatever isn't already stored locally. Assets are
/// fetched concurrently. Returns the manifest alongside a map of URL path ->
/// file bytes, ready to serve.
pub async fn fetch_app(
    client: &CanopeeClient,
    manifest_id: ObjectId,
    peer: Option<String>,
) -> anyhow::Result<(AppManifest, HashMap<String, Vec<u8>>)> {
    let manifest_object = get_or_fetch(client, &manifest_id, peer.as_deref()).await?;
    let manifest: AppManifest = manifest_object.decode()?;

    // Once we know who's serving the manifest, reuse that peer for every
    // other object instead of re-resolving a provider per asset.
    let peer = match peer {
        Some(peer) => Some(peer),
        None => client.find_providers(manifest_id).await?.into_iter().next(),
    };

    let entrypoint = get_or_fetch(client, &manifest.entrypoint, peer.as_deref()).await?;
    let mut files = HashMap::new();
    files.insert("/".to_string(), entrypoint.payload.data);

    let asset_fetches = manifest.assets.iter().map(|(path, object_id)| {
        let path = path.clone();
        let peer = peer.clone();
        async move {
            let object = get_or_fetch(client, object_id, peer.as_deref()).await?;
            Ok::<_, anyhow::Error>((path, object.payload.data))
        }
    });
    for (path, data) in try_join_all(asset_fetches).await? {
        files.insert(path, data);
    }

    Ok((manifest, files))
}

/// Serves `files` (URL path -> bytes) over plain HTTP on `127.0.0.1:port`
/// (pass 0 to let the OS pick a free port) until the process is killed.
pub async fn serve(files: HashMap<String, Vec<u8>>, port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await?;
    let files = Arc::new(files);

    println!("Serving app at http://{}", listener.local_addr()?);

    loop {
        let (stream, _) = listener.accept().await?;
        let files = files.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, &files).await {
                eprintln!("connection error: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    files: &HashMap<String, Vec<u8>>,
) -> anyhow::Result<()> {
    let path = {
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await? == 0 {
            return Ok(());
        }
        loop {
            let mut line = String::new();
            let n = reader.read_line(&mut line).await?;
            if n == 0 || line == "\r\n" || line == "\n" {
                break;
            }
        }
        request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or("/")
            .to_string()
    };

    let body = files.get(&path);
    let status = if body.is_some() {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let content_type = guess_content_type(&path);
    let content_length = body.map(|b| b.len()).unwrap_or(0);

    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(header.as_bytes()).await?;
    if let Some(body) = body {
        stream.write_all(body).await?;
    }
    Ok(())
}

fn guess_content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_index_and_assets() {
        let mut files = HashMap::new();
        files.insert("/".to_string(), b"<h1>hi</h1>".to_vec());
        files.insert("/style.css".to_string(), b"h1{color:red}".to_vec());

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let files = Arc::new(files);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let files = files.clone();
                tokio::spawn(async move {
                    handle_connection(stream, &files).await.unwrap();
                });
            }
        });

        let body = reqwest_get(&addr, "/").await;
        assert!(body.contains("hi"));
        let css = reqwest_get(&addr, "/style.css").await;
        assert!(css.contains("color:red"));
    }

    async fn reqwest_get(addr: &std::net::SocketAddr, path: &str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let response = String::from_utf8_lossy(&buf).to_string();
        response.split("\r\n\r\n").nth(1).unwrap_or("").to_string()
    }
}
