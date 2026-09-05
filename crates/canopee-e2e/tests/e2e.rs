//! End-to-end test: two real `canopee-node` processes (separate `$HOME`s),
//! driven through the real `canopee` CLI binary, over the real libp2p stack.
//!
//! Scenario:
//!   1. Nodes A and B start and discover each other via mDNS (localhost).
//!   2. A stores a file. It is private by default: B's fetch is refused.
//!   3. A claims the username `alice-e2e` (`canopee username claim`): B
//!      reverse-resolves it to A's identity (`username lookup`) and sees it
//!      in `canopee peers` instead of only A's raw peer id.
//!   4. A shares the file (`canopee share`): B discovers A as a DHT provider
//!      and fetches the object — by *username*, not raw peer id.
//!   5. A unshares it (`canopee unshare`): B's fetch is refused again.
//!
//! Run with:
//!   cargo test -p canopee-e2e -- --nocapture
//!
//! The test builds the `canopee-node`/`canopee` binaries itself (nested
//! `cargo build`) if they are missing or stale, so no manual build step is
//! required.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Overall guard so a hung node can never hang the test suite forever.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(180);
/// How long to wait for one node's Unix socket to appear after spawn.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
/// mDNS discovery + connection establishment on localhost.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);
/// DHT provider-record propagation between two directly connected peers.
const DHT_TIMEOUT: Duration = Duration::from_secs(60);

struct TestNode {
    home: PathBuf,
    child: Child,
    cli: PathBuf,
}

impl TestNode {
    fn spawn(node_bin: &Path, cli_bin: &Path, home: &Path) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let log = std::fs::File::create(home.join("node.log")).unwrap();
        let log_err = log.try_clone().unwrap();
        let child = Command::new(node_bin)
            .env("HOME", home)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .expect("failed to spawn canopee-node");

        let node = Self {
            home: home.to_path_buf(),
            child,
            cli: cli_bin.to_path_buf(),
        };
        node.wait_for_socket();
        node
    }

    fn wait_for_socket(&self) {
        let socket = self.home.join(".canopee/node.sock");
        let deadline = Instant::now() + SOCKET_TIMEOUT;
        while Instant::now() < deadline {
            if socket.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!(
            "node at {} never created its socket (see {})",
            self.home.display(),
            self.home.join("node.log").display()
        );
    }

    /// Runs `canopee <args>` against this node's `$HOME`, returning
    /// `(success, stdout, stderr)`.
    fn cli(&self, args: &[&str]) -> (bool, String, String) {
        let out = Command::new(&self.cli)
            .args(args)
            .env("HOME", &self.home)
            .output()
            .unwrap_or_else(|e| panic!("failed to run canopee {args:?}: {e}"));
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    fn cli_ok(&self, args: &[&str]) -> String {
        let (ok, stdout, stderr) = self.cli(args);
        assert!(ok, "canopee {args:?} failed: {stderr}");
        stdout
    }

    /// This node's own peer id, parsed from `canopee identity`'s output
    /// (`IdentityId("canopee://identity/<id>")`).
    fn own_peer_id(&self) -> String {
        let (ok, stdout, _) = self.cli(&["identity"]);
        assert!(ok, "canopee identity failed");
        let prefix = "canopee://identity/";
        let start = stdout
            .find(prefix)
            .unwrap_or_else(|| panic!("no {prefix} in identity output: {stdout}"));
        let rest = &stdout[start + prefix.len()..];
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        assert!(!id.is_empty(), "empty peer id in identity output: {stdout}");
        id
    }

    /// Waits until `other` is connected to `expected_peer` (B may also see
    /// the public bootstrap relay, so "first peer" is not good enough).
    fn wait_for_peer(&self, expected_peer: &str) {
        let deadline = Instant::now() + DISCOVERY_TIMEOUT;
        while Instant::now() < deadline {
            let (ok, stdout, _) = self.cli(&["peers"]);
            if ok && stdout.contains(expected_peer) {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        panic!(
            "node never discovered peer {expected_peer} (see {})",
            self.home.join("node.log").display()
        );
    }

    fn stop(&mut self) {
        let _ = self.cli(&["stop"]);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    return;
                }
            }
        }
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// Builds `canopee-node` and `canopee-cli` (idempotent — cargo's own
/// fingerprint check makes this cheap on re-runs) and returns their binary
/// paths. Note: the CLI binary is `canopee-cli`, not `canopee` — the latter
/// is the workspace root's stub package.
fn build_binaries() -> (PathBuf, PathBuf) {
    let root = workspace_root();
    let status = Command::new("cargo")
        .args(["build", "-p", "canopee-node", "-p", "canopee-cli"])
        .current_dir(&root)
        .status()
        .expect("failed to invoke cargo build");
    assert!(status.success(), "cargo build of e2e binaries failed");
    let debug = root.join("target/debug");
    (debug.join("canopee-node"), debug.join("canopee-cli"))
}

#[test]
fn two_nodes_share_and_unshare_end_to_end() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. mDNS discovery: B sees A specifically (the bootstrap relay's
        //    peer id is not an acceptable match).
        let peer_a = a.own_peer_id();
        b.wait_for_peer(&peer_a);

        // 2. A stores a file; it is private by default.
        let file = base.join("hello.txt");
        std::fs::write(&file, b"hello over the p2p wire").unwrap();
        let put_out = a.cli_ok(&["put", file.to_str().unwrap()]);
        let object_id = put_out
            .lines()
            .find_map(|l| {
                let t = l.trim();
                (t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())).then(|| t.to_string())
            })
            .unwrap_or_else(|| panic!("could not parse object id from: {put_out}"));

        // 3. Private by default: B's direct fetch is refused.
        let (ok, _, err) = b.cli(&["fetch", &peer_a, &object_id]);
        assert!(
            !ok,
            "fetching an unshared object must fail (stdout empty, stderr: {err})"
        );

        // 3b. Username registry: A claims a globally unique username; B
        //     reverse-resolves it through the DHT (registry record + verified
        //     `(owner, "username")` pointer), and `canopee peers` displays
        //     the name instead of only the raw peer id.
        a.cli_ok(&["username", "claim", "alice-e2e"]);
        let show = a.cli_ok(&["username", "show"]);
        assert!(
            show.trim() == "alice-e2e",
            "A's claimed username must round-trip: {show}"
        );
        let identity_a = format!("canopee://identity/{peer_a}");
        let deadline = Instant::now() + DHT_TIMEOUT;
        let resolved = loop {
            let (_, out, _) = b.cli(&["username", "lookup", "alice-e2e"]);
            if out.contains(&identity_a) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        assert!(
            resolved,
            "B never resolved username alice-e2e to {identity_a}"
        );
        let deadline = Instant::now() + DHT_TIMEOUT;
        let named = loop {
            let (_, out, _) = b.cli(&["peers"]);
            if out.contains("alice-e2e") {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        assert!(named, "B's `canopee peers` never showed A's username");

        // 4. A shares the file under a name.
        a.cli_ok(&["share", "hello.txt", &object_id]);

        // 5. B discovers A as a provider on the DHT and fetches the object.
        let deadline = Instant::now() + DHT_TIMEOUT;
        let found = loop {
            let (_, out, _) = b.cli(&["find-providers", &object_id]);
            if out.lines().any(|l| l.trim() == peer_a) {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        assert!(found, "B never saw A as a provider of the shared object");

        // Fetch by username, not by raw peer id: the CLI reverse-resolves
        // "alice-e2e" through the registry before fetching.
        b.cli_ok(&["fetch", "alice-e2e", &object_id]);
        let list = b.cli_ok(&["list"]);
        assert!(
            list.contains(&object_id),
            "B must hold the object after fetching: {list}"
        );

        // 6. A's home index shows the shared entry.
        let home = a.cli_ok(&["home"]);
        assert!(home.contains("hello.txt"), "home must list the entry: {home}");
        assert!(home.contains("Shared: yes"), "entry must be shared: {home}");

        // 7. Unshare: the gate closes again.
        a.cli_ok(&["unshare", "hello.txt"]);
        let home = a.cli_ok(&["home"]);
        assert!(home.contains("Shared: no"), "entry must be unshared: {home}");

        let (ok, _, _) = b.cli(&["fetch", &peer_a, &object_id]);
        // B already holds a copy, but A must refuse to serve it again.
        assert!(!ok, "fetching an unshared object must be refused");
    }));

    a.stop();
    b.stop();
    let _ = std::fs::remove_dir_all(&base);

    assert!(
        started.elapsed() < SCENARIO_TIMEOUT,
        "scenario exceeded its time budget"
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
