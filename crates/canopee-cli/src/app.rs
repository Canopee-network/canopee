use canopee_sdk::CanopeeClient;
use canopee_storage::{AppManifest, Object, ObjectId};
use futures::future::try_join_all;
use std::{collections::HashMap, path::Path, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub async fn publish_directory(
    client: &CanopeeClient,
    directory: &Path,
) -> anyhow::Result<(ObjectId, HashMap<String, ObjectId>)> {
    let mut assets = HashMap::new();
    let mut entrypoint = None;
    let mut entrypoint_html: Option<Vec<u8>> = None;

    for entry in walkdir::WalkDir::new(directory)
        .into_iter()
        .filter_entry(|e| !e.file_name().to_string_lossy().starts_with('.'))
    {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let data = tokio::fs::read(path).await.unwrap();
        let object_id = client.put_file(data.clone()).await?;
        let relative = path.strip_prefix(directory)?.to_string_lossy().to_string();

        println!("{} -> {}", relative, object_id);

        if relative == "index.html" {
            entrypoint = Some(object_id.clone());
            entrypoint_html = Some(data);
        } else {
            assets.insert(format!("/{}", relative), object_id);
        }
    }
    let entrypoint = entrypoint.ok_or(anyhow::anyhow!("portfolio has no index.html"))?;

    // Stretch step: a build with a non-root `base`/`homepage` config
    // references assets under a prefix publish_directory never created
    // (e.g. `/my-app/assets/...` instead of `/assets/...`). Catch that at
    // publish time rather than shipping a site that 404s every asset.
    if let Some(html) = entrypoint_html.as_deref() {
        if let Ok(html) = std::str::from_utf8(html) {
            let missing = unpublished_asset_references(html, &assets);
            if !missing.is_empty() {
                anyhow::bail!(
                    "index.html references paths that were not published (maybe a non-root \
                     build base?): {} — rebuild with `base: '/'` (Vite) or unset `homepage` \
                     (CRA) so assets are referenced from the repository root",
                    missing.join(", ")
                );
            }
        }
    }

    Ok((entrypoint, assets))
}

/// Scans `index.html` for absolute `src="/..."` / `href="/..."` references
/// and returns those with no matching published asset path. A non-root build
/// base makes the HTML point at e.g. `/my-app/assets/x.js` while
/// `publish_directory` only publishes `/assets/x.js`.
fn unpublished_asset_references(
    html: &str,
    assets: &HashMap<String, ObjectId>,
) -> Vec<String> {
    let mut missing = Vec::new();
    for value in find_absolute_urls(html) {
        let key = value
            .split(['?', '#'])
            .next()
            .unwrap_or(value.as_str())
            .to_string();
        if !assets.contains_key(&key) && !missing.contains(&key) {
            missing.push(key);
        }
    }
    missing
}

/// Returns paths from `src="/..."` and `href="/..."` attributes in HTML (a
/// simple substring search covers real bundler output).
fn find_absolute_urls(html: &str) -> Vec<String> {
    let mut found = Vec::new();
    for attr in ["src=", "href="] {
        let mut rest = html;
        while let Some(rel) = rest.find(attr) {
            let after_attr = &rest[rel + attr.len()..];
            // Skip quoted/unquoted: bundlers emit `src="/..."`; handle a
            // leading quote char if present.
            let after_quote = after_attr
                .strip_prefix(['"', '\''])
                .unwrap_or(after_attr);
            let value = after_quote
                .split(|c: char| c == '"' || c == '\'' || c == '>' || c.is_whitespace())
                .next()
                .unwrap_or("");
            if value.starts_with('/') && !value.starts_with("//") {
                found.push(value.to_string());
            }
            rest = after_attr;
        }
    }
    found
}

/// Resolves peers that can serve `id`: returns the explicit `peer` hint
/// first (if given), then every provider found on the DHT for `id`.
async fn resolve_providers(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let mut providers: Vec<String> = Vec::new();
    if let Some(peer) = peer {
        providers.push(peer.to_string());
    }
    let dht_providers = client.find_providers(id.clone()).await?;
    for p in dht_providers {
        if !providers.contains(&p) {
            providers.push(p);
        }
    }
    Ok(providers)
}

/// Reads an object from local storage, falling back to fetching it from a
/// peer (trying multiple candidates if the first is offline) and importing
/// it locally. After a successful import from a remote peer, announces
/// this node as an additional provider and marks the object as cached.
async fn get_or_fetch(
    client: &CanopeeClient,
    id: &ObjectId,
    peer: Option<&str>,
) -> anyhow::Result<Object> {
    if let Ok(object) = client.get(id.clone()).await {
        return Ok(object);
    }

    let providers = resolve_providers(client, id, peer).await?;
    if providers.is_empty() {
        anyhow::bail!("no providers found for {id}; try passing --peer");
    }

    let mut last_err = anyhow::anyhow!("no providers available");
    for peer_id in &providers {
        match client.fetch_object(peer_id, id.clone()).await {
            Ok(bundle) => {
                // Import (the node auto-marks non-owned objects as cached)
                // then announce as a provider — best-effort: a failed announce
                // should not fail the fetch itself.
                client.import(bundle).await?;
                if let Err(e) = client.announce(id.clone()).await {
                    eprintln!("Warning: failed to announce {id} as provider: {e}");
                }
                return client.get(id.clone()).await;
            }
            Err(e) => {
                last_err = e;
                continue;
            }
        }
    }
    Err(last_err)
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

    // Use the `peer` hint (if given) for every referenced object; otherwise
    // let each object resolve its own provider set from the DHT (a cache node
    // holding one asset may not hold another).
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
            .split('?')
            .next()
            .unwrap_or("/")
            .to_string()
    };

    // Exact match wins. Otherwise, SPA fallback: a miss for a route-shaped
    // path (no file extension in its last segment) serves the entrypoint so
    // the client-side router can render `/about` etc. on refresh/direct
    // load. A miss for something that *looks like an asset* (e.g.
    // `/assets/broken.js`) stays a real 404, so a broken asset reference
    // isn't silently swaddled in HTML. This deliberately ignores the HTTP
    // method (heads, POSTs, etc. are treated the same) — the server is a
    // read-only static host and parsing methods is out of scope here.
    let (served_key, body, status) = if let Some(body) = files.get(&path) {
        (&path as &str, Some(body), "200 OK")
    } else if !is_asset_request(&path) {
        match files.get("/") {
            Some(entrypoint) => ("/", Some(entrypoint), "200 OK"),
            None => (&path as &str, None, "404 Not Found"),
        }
    } else {
        (&path as &str, None, "404 Not Found")
    };

    // Content type is derived from the key actually served, not the
    // requested route — so a fallback to the entrypoint gets `text/html`
    // even though the browser asked for `/about`.
    let content_type = guess_content_type(served_key);
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
    // The entrypoint is served with a path of "/", which has no extension —
    // resolve it to `index.html` so it gets a real HTML content type.
    let name = if path == "/" { "index.html" } else { path };
    match name.rsplit('.').next() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css",
        Some("js") | Some("mjs") => "application/javascript",
        Some("json") => "application/json",
        Some("map") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("eot") => "application/vnd.ms-fontobject",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        _ => "application/octet-stream",
    }
}

/// True when a request path's last segment carries a file extension, i.e. it
/// is asking for a specific *asset* rather than a client-side route. Used to
/// decide whether a cache miss should fall back to the SPA entrypoint.
fn is_asset_request(path: &str) -> bool {
    let last = path.rsplit('/').next().unwrap_or("");
    last.contains('.')
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

        let (status, body) = http_request(&addr, "/").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(body.contains("hi"));
        let (status, css) = http_request(&addr, "/style.css").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(css.contains("color:red"));
    }

    #[tokio::test]
    async fn spa_fallback_serves_entrypoint_for_routes_and_404s_missing_assets() {
        let mut files = HashMap::new();
        files.insert("/".to_string(), b"<div id=root>app</div>".to_vec());
        files.insert("/assets/app.js".to_string(), b"console.log('hi')".to_vec());

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

        // Route-shaped path missing from `files` -> served the entrypoint.
        let (status, content_type, body) = http_request_with_ct(&addr, "/about").await;
        assert!(status.contains(" 200 "), "route should fall back to index: {status}");
        assert!(body.contains("app"));
        assert!(content_type.starts_with("text/html"), "fallback body must be marked html, got {content_type}");

        // Nested route with a query string.
        let (status, body) = http_request(&addr, "/users/42?tab=posts").await;
        assert!(status.contains(" 200 "), "got {status}");
        assert!(body.contains("app"));

        // Missing *asset-looking* path stays a real 404.
        let (status, _content_type, _body) =
            http_request_with_ct(&addr, "/assets/does-not-exist.js").await;
        assert!(status.contains(" 404 "), "missing asset must not silently fall back");
    }

    #[test]
    fn content_types_cover_common_bundler_output() {
        for (path, expected) in [
            ("/", "text/html; charset=utf-8"),
            ("/index.html", "text/html; charset=utf-8"),
            ("/style.css", "text/css"),
            ("/app.js", "application/javascript"),
            ("/app.mjs", "application/javascript"),
            ("/app.json", "application/json"),
            ("/app.js.map", "application/json"),
            ("/LICENSE.txt", "text/plain; charset=utf-8"),
            ("/img.png", "image/png"),
            ("/img.webp", "image/webp"),
            ("/img.avif", "image/avif"),
            ("/img.gif", "image/gif"),
            ("/img.svg", "image/svg+xml"),
            ("/favicon.ico", "image/x-icon"),
            ("/font.woff", "font/woff"),
            ("/font.woff2", "font/woff2"),
            ("/font.ttf", "font/ttf"),
            ("/font.otf", "font/otf"),
            ("/font.eot", "application/vnd.ms-fontobject"),
            ("/unknown.zzz", "application/octet-stream"),
        ] {
            assert_eq!(guess_content_type(path), expected, "mismatch for {path}");
        }
    }

    #[test]
    fn is_asset_request_distinguishes_routes_from_assets() {
        assert!(!is_asset_request("/"));
        assert!(!is_asset_request("/about"));
        assert!(!is_asset_request("/users/42"));
        assert!(!is_asset_request("/users/42?tab=posts"));
        assert!(!is_asset_request("/about/"));
        assert!(is_asset_request("/assets/app.js"));
        assert!(is_asset_request("/assets/app.js.map"));
        assert!(is_asset_request("/favicon.ico"));
    }

    #[test]
    fn publish_catches_non_root_build_base() {
        // HTML referencing assets under a base path that was never published.
        let html = r#"
            <html>
              <link rel="icon" href="/my-app/favicon.ico">
              <script src="/my-app/assets/index-a1b2c3.js"></script>
              <script src="//external.example/vendor.js"></script>
            </html>
        "#;
        let published = vec![
            ("/assets/index-a1b2c3.js".to_string(), ObjectId::new("/assets/index-a1b2c3.js")),
            ("/favicon.ico".to_string(), ObjectId::new("/favicon.ico")),
        ]
        .into_iter()
        .collect::<HashMap<_, _>>();

        // A correct build (root-relative refs all published) passes.
        let ok_html = r#"<script src="/assets/index-a1b2c3.js"></script>"#;
        assert!(unpublished_asset_references(ok_html, &published).is_empty());

        let missing = unpublished_asset_references(html, &published);
        assert_eq!(
            missing,
            vec!["/my-app/assets/index-a1b2c3.js", "/my-app/favicon.ico"]
        );
    }

    async fn raw_get(addr: &std::net::SocketAddr, path: &str) -> String {
        use tokio::io::AsyncReadExt;
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Returns (status line, content-type, body) from a raw HTTP response.
    fn parse_response(response: &str) -> (String, String, String) {
        let mut parts = response.splitn(2, "\r\n\r\n");
        let head = parts.next().unwrap_or("");
        let body = parts.next().unwrap_or("").to_string();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or("").to_string();
        let content_type = lines
            .find_map(|line| line.strip_prefix("Content-Type:"))
            .unwrap_or("")
            .trim()
            .to_string();
        (status, content_type, body)
    }

    async fn http_request(addr: &std::net::SocketAddr, path: &str) -> (String, String) {
        let (status, _, body) = parse_response(&raw_get(addr, path).await);
        (status, body)
    }

    async fn http_request_with_ct(
        addr: &std::net::SocketAddr,
        path: &str,
    ) -> (String, String, String) {
        parse_response(&raw_get(addr, path).await)
    }
}
