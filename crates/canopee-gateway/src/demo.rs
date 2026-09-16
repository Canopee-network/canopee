//! The small self-contained demo page served from `http://127.0.0.1:PORT/`.
//! It embeds the browser client (`www/client.js`) and the session token, then
//! connects to its own origin via WebSocket — same-origin, loopback, token
//! injected server-side.

const CLIENT_JS: &str = include_str!("../www/client.js");
const PAGE_TEMPLATE: &str = include_str!("../www/demo.html");

/// Builds the demo page for the given session token. The token is injected
/// directly into the page so a visitor never has to type or paste it — it is
/// only ever sent to a loopback address.
pub fn demo_page(token: &str) -> String {
    PAGE_TEMPLATE
        .replace("__CANOPEE_TOKEN__", token)
        .replace("__CLIENT_JS__", CLIENT_JS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_page_embeds_token_and_client() {
        let page = demo_page("ab12cd34");
        assert!(page.contains("ab12cd34"));
        assert!(page.contains("class CanopeeWeb"));
        assert!(!page.contains("__CLIENT_JS__"));
        assert!(!page.contains("__CANOPEE_TOKEN__"));
    }
}
