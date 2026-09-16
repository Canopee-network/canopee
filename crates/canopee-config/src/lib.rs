use std::path::PathBuf;

/// Split-root configuration: the *app root* holds per-app runtime state
/// (network state, cache, socket, exports), while the *user root* holds
/// everything that belongs to the person rather than any one app — their
/// identity key, their object store, their resolved record cache, and their
/// alias name map.
///
/// The model: **state is per-app, data is per-user.** Every Canopee app
/// pointed at the same user root shares the same identity and the same
/// objects; nothing per-app is visible to any other app.
///
/// Backward compatibility: `Config::new()` and `Config::with_root` keep both
/// roots equal (the CLI node and single-root embedded apps behave exactly as
/// before). Embedded apps that want cross-app data sharing use
/// `Config::new().with_app_root(app_dir)` — identity + storage stay at
/// `~/.canopee`, everything app-specific goes under `app_dir`.
///
/// Instance isolation: `Config::new()` honors the `CANOPEE_APP_ROOT`
/// environment variable, which replaces `dirs::home_dir()` as the base under
/// which the `.canopee` app root lives. Both roots stay equal, so the node,
/// the SDK client and the CLI all resolve the same socket/identity/storage —
/// a scriptable equivalent of launching an instance with a custom `HOME` (used
/// to run several isolated node+GUI instances side by side on one machine).
///
/// LAN simulation: set `CANOPEE_MDNS=0` to disable multicast discovery, so the
/// instance only finds peers through the Kademlia DHT (bootstrap + dialing) —
/// the same path two devices on *different* networks use. Combine with
/// `CANOPEE_BOOTSTRAP_ADDRS` (`canopee-network`) to point the instance at a
/// private bootstrap node instead of the public relay.
#[derive(Clone)]
pub struct Config {
    /// Per-app root (runtime state, socket, exports).
    root: PathBuf,
    /// Shared per-user root (identity, storage, records, aliases).
    user_root: PathBuf,
    /// Whether the network announces/discears the identity over multicast DNS.
    /// The CLI node keeps it on (default); embedded apps sharing an identity
    /// across processes should disable it so a second app's swarm doesn't
    /// re-announce the same `PeerId` over mDNS while the first is still live
    /// (they still discover each other via Kademlia/bootstrap + dialing).
    mdns_enabled: bool,
}

impl Config {
    pub fn new() -> Self {
        let base = std::env::var_os("CANOPEE_APP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs::home_dir().unwrap());
        let root = base.join(".canopee");
        // `CANOPEE_MDNS=0` (also `false`, `no`, `off`) turns multicast
        // discovery off (default on), so an instance only reaches peers over
        // Kademlia/bootstrap + dialing — the path used between devices that
        // are NOT on the same LAN.
        let mdns_enabled = match std::env::var("CANOPEE_MDNS").ok() {
            Some(v) => !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"),
            None => true,
        };
        Self {
            root: root.clone(),
            user_root: root,
            mdns_enabled,
        }
    }

    /// A `Config` rooted at an explicit directory instead of `~/.canopee`.
    ///
    /// Both the per-app root and the shared user root point at `root`, so
    /// anything built on `with_root` keeps its fully isolated
    /// identity/storage/socket behavior unchanged — the classic embedded-app
    /// (or test) setup where two instances with different roots never
    /// collide.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root: root.clone(),
            user_root: root,
            mdns_enabled: true,
        }
    }

    /// Keeps the shared user root (identity, storage, records) where it is
    /// and moves only this app's runtime state to `app_root`.
    pub fn with_app_root(mut self, app_root: PathBuf) -> Self {
        self.root = app_root;
        self
    }

    /// Points the shared user root at `user_root` instead of `~/.canopee`. The
    /// per-app root is unchanged.
    pub fn with_user_root(mut self, user_root: PathBuf) -> Self {
        self.user_root = user_root;
        self
    }

    /// Fully explicit split: app runtime state under `root`, shared user data
    /// under `user_root`. Used by tests that need a hermetic user root too.
    pub fn with_roots(mut self, root: PathBuf, user_root: PathBuf) -> Self {
        self.root = root;
        self.user_root = user_root;
        self
    }

    /// Enables/disables multicast DNS discovery. Default: on. Embedded apps
    /// that share one identity across multiple processes should call
    /// `.with_mdns(false)`.
    pub fn with_mdns(mut self, mdns_enabled: bool) -> Self {
        self.mdns_enabled = mdns_enabled;
        self
    }

    /// The per-app root: home of runtime state, socket, and exports.
    pub fn home_dir(&self) -> PathBuf {
        self.root.clone()
    }

    /// The shared per-user root: identity, storage, records, aliases.
    pub fn user_root(&self) -> PathBuf {
        self.user_root.clone()
    }

    pub fn mdns_enabled(&self) -> bool {
        self.mdns_enabled
    }

    /// Directory containing the user's identity key file. Lives under the
    /// *user* root so every app of the same person resolves to the same
    /// identity.
    pub fn identity_path(&self) -> PathBuf {
        self.user_root.join("identity")
    }

    /// The per-device key file, stored alongside the shared identity key. The
    /// identity key (`identity_path()/identity.key`) is identical on every
    /// device; this one is distinct on each, so the libp2p `PeerId` a device
    /// announces and the signing identity it represents stay independent —
    /// several devices can share one identity without colliding on the
    /// network. Never leaves the device it was created on.
    pub fn device_key_path(&self) -> PathBuf {
        self.identity_path().join("device.key")
    }

    /// The user's object store (their `Object`s: files, pictures, contacts,
    /// profile, home index). Shared across all the user's apps.
    pub fn storage_path(&self) -> PathBuf {
        self.user_root.join("storage")
    }

    /// Local cache of resolved `(owner, name)` records, so an app can see
    /// another app's freshly published pointers on the same machine without
    /// waiting for DHT round trips.
    pub fn records_path(&self) -> PathBuf {
        self.user_root.join("records")
    }

    pub fn export_path(&self) -> PathBuf {
        self.root.join("exports")
    }

    pub fn node_socket_path(&self) -> PathBuf {
        self.root.join("node.sock")
    }

    /// Map of friendly short names (`alice`) to canonical owner identities
    /// (`canopee://identity/<peer-id>`), used to resolve `canopee://` URIs.
    /// User-scoped: the names a person uses for owners are theirs, not the
    /// app's.
    pub fn aliases_path(&self) -> PathBuf {
        self.user_root.join("aliases")
    }

    /// Scratch file where the `canopee://` scheme handler records the URI it
    /// was handed when no node was running, so it isn't silently dropped.
    pub fn uri_pending_path(&self) -> PathBuf {
        self.root.join("uri-pending")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_root_keeps_single_root_backward_compat() {
        let c = Config::with_root(PathBuf::from("/tmp/app"));
        assert_eq!(c.home_dir(), PathBuf::from("/tmp/app"));
        assert_eq!(c.user_root(), PathBuf::from("/tmp/app"));
        assert_eq!(c.identity_path(), PathBuf::from("/tmp/app/identity"));
        assert_eq!(c.storage_path(), PathBuf::from("/tmp/app/storage"));
        assert!(c.mdns_enabled());
    }

    #[test]
    fn app_root_split_keeps_user_paths_at_user_root() {
        let c = Config::new()
            .with_app_root(PathBuf::from("/tmp/app"))
            .with_mdns(false);
        // App-scoped state moves with the app root.
        assert_eq!(c.home_dir(), PathBuf::from("/tmp/app"));
        assert_eq!(c.state_path(), PathBuf::from("/tmp/app/state/node.state"));
        assert_eq!(c.node_socket_path(), PathBuf::from("/tmp/app/node.sock"));
        // User-scoped data stays at ~/.canopee.
        let user = dirs::home_dir().unwrap().join(".canopee");
        assert_eq!(c.identity_path(), user.join("identity"));
        assert_eq!(c.storage_path(), user.join("storage"));
        assert_eq!(c.records_path(), user.join("records"));
        assert_eq!(c.aliases_path(), user.join("aliases"));
        assert!(!c.mdns_enabled());
    }

    #[test]
    fn with_roots_splits_both_explicitly() {
        let c = Config::new().with_roots(
            PathBuf::from("/tmp/app-a"),
            PathBuf::from("/tmp/user"),
        );
        assert_eq!(c.home_dir(), PathBuf::from("/tmp/app-a"));
        assert_eq!(c.identity_path(), PathBuf::from("/tmp/user/identity"));
        assert_eq!(c.storage_path(), PathBuf::from("/tmp/user/storage"));
    }

    #[test]
    fn with_user_root_moves_only_the_user_scope() {
        let c = Config::with_root(PathBuf::from("/tmp/app")).with_user_root(PathBuf::from("/tmp/user"));
        assert_eq!(c.home_dir(), PathBuf::from("/tmp/app"));
        assert_eq!(c.identity_path(), PathBuf::from("/tmp/user/identity"));
        assert_eq!(c.storage_path(), PathBuf::from("/tmp/user/storage"));
        assert_eq!(c.state_path(), PathBuf::from("/tmp/app/state/node.state"));
    }

    #[test]
    fn device_key_sits_beside_the_identity_key_file() {
        let c = Config::new();
        assert_eq!(
            c.device_key_path(),
            c.identity_path().join("device.key"),
            "device key lives next to identity.key, not inside it"
        );
        assert_ne!(c.device_key_path(), c.identity_path().join("identity.key"));
    }

    /// Runs in a child process (env overrides must never leak into the
    /// parallel test harness): peers at the resolved paths of `Config::new()`
    /// while `CANOPEE_APP_ROOT` is set. Skipped in the ordinary harness run —
    /// only the parent `env_override_names_an_isolated_instance` re-invokes it
    /// (via `--ignored --exact`) with the variable set.
    #[test]
    #[ignore]
    fn assert_env_override_paths() {
        let c = Config::new();
        let root = PathBuf::from("/tmp/env-root/.canopee");
        assert_eq!(c.home_dir(), root);
        assert_eq!(c.user_root(), root);
        assert_eq!(c.node_socket_path(), root.join("node.sock"));
        assert_eq!(c.identity_path(), root.join("identity"));
        assert_eq!(c.storage_path(), root.join("storage"));
    }

    #[test]
    fn env_override_names_an_isolated_instance() {
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(&exe)
            .arg("--exact")
            .arg("canopee_config::tests::assert_env_override_paths")
            .arg("--ignored")
            .arg("--nocapture")
            .env("CANOPEE_APP_ROOT", "/tmp/env-root")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "child assertions failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Runs in a child process, like `assert_env_override_paths`: asserts
    /// `CANOPEE_MDNS=0` turns multicast discovery off in `Config::new()`.
    #[test]
    #[ignore]
    fn assert_env_mdns_override() {
        assert!(!Config::new().mdns_enabled());
    }

    #[test]
    fn env_mdns_toggle_names_an_isolated_instance() {
        let exe = std::env::current_exe().unwrap();
        for value in ["0", "false", "none"] {
            let out = std::process::Command::new(&exe)
                .arg("--exact")
                .arg("canopee_config::tests::assert_env_mdns_override")
                .arg("--ignored")
                .arg("--nocapture")
                .env("CANOPEE_MDNS", value)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "CANOPEE_MDNS={value} must disable mdns ({})",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let out = std::process::Command::new(&exe)
            .arg("--exact")
            .arg("canopee_config::tests::assert_env_mdns_survives_by_default")
            .arg("--ignored")
            .arg("--nocapture")
            .output()
            .unwrap();
        assert!(out.status.success());
    }

    /// Runs in a child process: without `CANOPEE_MDNS`, mdns stays on.
    #[test]
    #[ignore]
    fn assert_env_mdns_survives_by_default() {
        assert!(Config::new().mdns_enabled());
    }
}