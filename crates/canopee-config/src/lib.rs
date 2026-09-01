use std::path::PathBuf;

pub struct Config {
    root: PathBuf,
}

impl Config {
    pub fn new() -> Self {
        Self {
            root: dirs::home_dir().unwrap().join(".canopee"),
        }
    }

    pub fn home_dir(&self) -> PathBuf {
        self.root.clone()
    }

    pub fn identity_path(&self) -> PathBuf {
        self.root.join("identity")
    }

    pub fn storage_path(&self) -> PathBuf {
        self.root.join("storage")
    }

    pub fn export_path(&self) -> PathBuf {
        self.root.join("exports")
    }

    pub fn node_socket_path(&self) -> PathBuf {
        self.root.join("node.sock")
    }

    pub fn listen_addr(&self) -> String {
        let port = std::env::var("CANOPEE_LISTEN_PORT").unwrap_or_else(|_| "0".to_string());
        format!("/ip4/0.0.0.0/tcp/{port}")
    }

    pub fn state_path(&self) -> PathBuf {
        self.home_dir().join("state").join("node.state")
    }

    /// Path to the cache index sidecar recording which locally stored objects
    /// were fetched from other peers (vs. created by this node's identity)
    /// and when each was last served, so the node can safely evict least
    /// recently used cached objects without ever touching its own.
    pub fn cache_path(&self) -> PathBuf {
        self.home_dir().join("cache.cache")
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}
