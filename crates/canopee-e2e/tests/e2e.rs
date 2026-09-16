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
//! IMPORTANT: e2e tests must run serially (`--test-threads=1`) — parallel
//! execution lets nodes from different tests cross-connect over mDNS/DHT and
//! interfere nondeterministically. Serial execution is the supported mode:
//!   cargo test -p canopee-e2e -- --test-threads=1
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

    /// This node's canonical identity string (`canopee://identity/<peer-id>`),
    /// parsed from `canopee identity` output.
    fn identity_id(&self) -> String {
        let prefix = "canopee://identity/";
        let stdout = self.cli_ok(&["identity"]);
        let start = stdout
            .find(prefix)
            .unwrap_or_else(|| panic!("no {prefix} in identity output: {stdout}"));
        let id: String = stdout[start..]
            .chars()
            .take_while(|c| *c != '"' && *c != ')' && !c.is_whitespace())
            .collect();
        assert!(
            id.starts_with(prefix),
            "malformed identity in output: {stdout}"
        );
        id
    }

    /// This machine's *device* `PeerId` (first line of `canopee device`), the
    /// network identity announced by the node's swarm — since device keys,
    /// this is distinct from the identity string's embedded peer id.
    fn device_peer_id(&self) -> String {
        let stdout = self.cli_ok(&["device"]);
        let id: String = stdout
            .lines()
            .next()
            .expect("canopee device printed no device id")
            .trim()
            .to_string();
        assert!(
            id.starts_with("12D3KooW"),
            "malformed device peer id in output: {stdout}"
        );
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
        //    peer id is not an acceptable match). A's *device* peer id is its
        //    network identity now — the identity string's embedded peer id is
        //    no longer dialable.
        let device_a = a.device_peer_id();
        let identity_a = a.identity_id();
        assert_ne!(
            device_a,
            a.own_peer_id(),
            "the device peer id must differ from the identity-bound peer id"
        );
        b.wait_for_peer(&device_a);

        // 2. A stores a file; it is private by default. The id is normally
        //    hidden — ask for it explicitly with `--ids` (the default `put`
        //    output is just `Stored <name>`).
        let file = base.join("hello.txt");
        std::fs::write(&file, b"hello over the p2p wire").unwrap();
        let put_out = a.cli_ok(&["put", file.to_str().unwrap(), "--ids"]);
        let object_id = put_out
            .lines()
            .find_map(|l| {
                let t = l.trim().strip_prefix("Id: ").unwrap_or(l.trim());
                (t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit())).then(|| t.to_string())
            })
            .unwrap_or_else(|| panic!("could not parse object id from: {put_out}"));

        // 3. Private by default: B's direct fetch is refused. Fetching by
        //    A's identity resolves to A's registered device (the Phase-2
        //    identity → device path), which refuses an unshared object.
        let (ok, _, err) = b.cli(&["fetch", &identity_a, &object_id]);
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
            if out.lines().any(|l| l.trim() == device_a) {
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
        let list = b.cli_ok(&["list", "--ids"]);
        assert!(
            list.contains(&object_id),
            "B must hold the object after fetching: {list}"
        );

        // 5b. Fetch by *name*, ids never needed: B resolves A's
        //     `(owner, "entry:hello.txt")` pointer on the DHT and dials A's
        //     device directly. Waiting until it resolves avoids racing the
        //     fire-and-forget DHT publication from `share`.
        let deadline = Instant::now() + DHT_TIMEOUT;
        let name_fetched = loop {
            let (ok, out, err) = b.cli(&["fetch", "alice-e2e", "hello.txt"]);
            if ok && out.contains("Fetched \"hello.txt\"") {
                break true;
            }
            if Instant::now() >= deadline {
                eprintln!("name-fetch stderr: {err}");
                break false;
            }
            std::thread::sleep(Duration::from_secs(1));
        };
        assert!(name_fetched, "B never fetched hello.txt by name");
        let list = b.cli_ok(&["list", "--ids"]);
        assert!(
            list.contains("hello.txt") && list.contains(&object_id),
            "the name-fetched object must list by name (and id with --ids): {list}"
        );

        // 6. A's home index shows the shared entry.
        let home = a.cli_ok(&["home"]);
        assert!(home.contains("hello.txt"), "home must list the entry: {home}");
        assert!(home.contains("Shared: yes"), "entry must be shared: {home}");

        // 7. Unshare: the gate closes again.
        a.cli_ok(&["unshare", "hello.txt"]);
        let home = a.cli_ok(&["home"]);
        assert!(home.contains("Shared: no"), "entry must be unshared: {home}");

        let (ok, _, _) = b.cli(&["fetch", &identity_a, &object_id]);
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

/// Phase 1 of multi-device identity: move an identity from one device to
/// another via an encrypted export file.
///
/// Scenario:
///   1. A exports its identity (`canopee export-identity`) to an encrypted
///      file.
///   2. B refuses to import it over its own fresh identity (`--overwrite`
///      unset), and rejects a wrong passphrase.
///   3. B imports A's identity (`--overwrite` set): the live node still shows
///      its old identity, but the key file on disk now holds A's.
///   4. After B restarts, both nodes report the same `IdentityId`.
#[test]
fn two_nodes_export_and_import_identity() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_identity_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");
    let export_path = base.join("identity-a.bin");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let identity_a = a.identity_id();
    let identity_b_origin = b.identity_id();
    let device_a = a.device_peer_id();
    let device_b_origin = b.device_peer_id();
    assert_ne!(
        device_a, device_b_origin,
        "two fresh nodes must have distinct device peer ids"
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_ne!(
            identity_a, identity_b_origin,
            "two fresh nodes must start with different identities"
        );

        // 1. A exports its identity to an encrypted file.
        a.cli_ok(&[
            "export-identity",
            "--passphrase",
            "swordfish",
            "--output",
            export_path.to_str().unwrap(),
        ]);
        assert!(export_path.exists(), "export file must be written");

        // 2. B refuses to clobber its own identity without --overwrite.
        let (ok, _, stderr) = b.cli(&[
            "import-identity",
            export_path.to_str().unwrap(),
            "--passphrase",
            "swordfish",
        ]);
        assert!(
            !ok,
            "import without --overwrite must refuse an existing identity"
        );
        assert!(
            stderr.contains("already exists"),
            "refusal should explain why: {stderr}"
        );

        // 3. A wrong passphrase is rejected even with --overwrite.
        let (ok, _, _) = b.cli(&[
            "import-identity",
            export_path.to_str().unwrap(),
            "--passphrase",
            "wrong",
            "--overwrite",
        ]);
        assert!(!ok, "wrong passphrase must fail to import");

        // 4. A proper import succeeds, reports A's identity, and flags the
        //    restart required to adopt it.
        let out = b.cli_ok(&[
            "import-identity",
            export_path.to_str().unwrap(),
            "--passphrase",
            "swordfish",
            "--overwrite",
        ]);
        assert!(
            out.contains(&identity_a),
            "import must report A's identity: {out}"
        );
        assert!(
            out.contains("Restart"),
            "import must flag the needed restart: {out}"
        );

        // 5. The live node is unchanged until restart...
        assert_eq!(
            b.identity_id(),
            identity_b_origin,
            "the running node keeps its old identity until restart"
        );
        b.stop();
    }));

    // 6. After restart, B adopts exactly A's identity, but keeps its own
    //    device key — a device key never leaves the machine it was minted on.
    if result.is_ok() {
        let mut b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        let identity_b_after = b2.identity_id();
        let device_b_after = b2.device_peer_id();
        assert_eq!(
            identity_b_after, identity_a,
            "after restart B must be A's identity"
        );
        assert_eq!(
            device_b_after, device_b_origin,
            "importing a new identity must not change this device's network peer id"
        );
        assert_ne!(
            device_b_after, device_a,
            "B's device must remain distinct from A's"
        );
        // Each node lists its own device in its local device list.
        let devs_a = a.cli_ok(&["devices"]);
        assert!(
            devs_a.contains(&device_a),
            "A's device list must include its own device: {devs_a}"
        );
        let devs_b2 = b2.cli_ok(&["devices"]);
        assert!(
            devs_b2.contains(&device_b_after),
            "B2's device list must include its own device: {devs_b2}"
        );
        b2.stop();
    }

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

/// Phase 3 of multi-device identity: pair a new device (B) onto an existing
/// identity (A) over the LAN, using the 12-char pairing code as the explicit
/// approval. Identity + signed user records travel encrypted; the code itself
/// never crosses the network.
///
/// Scenario:
///   1. A and B start as two fresh devices with different identities.
///   2. B runs `canopee pair` (new-device side): prints a pairing code and a
///      QR payload containing its session id + dialable LAN address.
///   3. A runs `canopee pair <payload> --code <code>` (source side): verifies
///      the typed code, encrypts its identity + device list, dials B on the
///      LAN over `/canopee/pairing/1.0.0` and delivers the payload. B accepts.
///   4. B keeps running its temporary identity until restart, then adopts A's
///      identity exactly — while keeping its own device key / peer id.
///   5. B's device list now carries both A's device and its own.
#[test]
fn two_nodes_pair_over_lan() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_pair_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let identity_a = a.identity_id();
    let identity_b = b.identity_id();
    let device_a = a.device_peer_id();
    let device_b_origin = b.device_peer_id();
    assert_ne!(identity_a, identity_b, "two fresh nodes have distinct identities");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. New-device side: B prints a pairing code + QR payload.
        let out = b.cli_ok(&["pair"]);
        let code = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("Pairing code: ").map(str::to_string))
            .unwrap_or_else(|| panic!("no pairing code in: {out}"));
        assert_eq!(code.len(), 12, "pairing code must be 12 chars: {code}");
        let payload = out
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("canopee pair ")
                    .map(|s| s.split(" --code").next().unwrap().to_string())
            })
            .unwrap_or_else(|| panic!("no qr payload in: {out}"));

        // 2. Source side: A verifies the typed code and delivers the identity
        //    over the LAN. Success proves the code matched, B was dialable at
        //    its advertised LAN address, and B accepted.
        let out = a.cli_ok(&["pair", &payload, "--code", &code]);
        assert!(
            out.to_lowercase().contains("accepted"),
            "A must report B's acceptance: {out}"
        );

        // 3. A wrong code is rejected at the source, before anything is sent.
        let (ok, _, stderr) = a.cli(&["pair", &payload, "--code", "WRONGWRONGWRONG"]);
        assert!(!ok, "a wrong pairing code must be rejected");
        assert!(
            stderr.to_lowercase().contains("does not match"),
            "rejection should explain the mismatch: {stderr}"
        );

        // 4. The running node keeps its temporary identity until restart.
        assert_eq!(
            b.identity_id(),
            identity_b,
            "the live node keeps its identity until restart"
        );
        b.stop();
    }));

    // 5. After restart, B adopts exactly A's identity but keeps its device key.
    if result.is_ok() {
        let mut b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        let identity_b_after = b2.identity_id();
        let device_b_after = b2.device_peer_id();
        assert_eq!(
            identity_b_after, identity_a,
            "after pairing, B must be exactly A's identity"
        );
        assert_eq!(
            device_b_after, device_b_origin,
            "pairing must not change this device's network peer id"
        );
        assert_ne!(
            device_b_after, device_a,
            "B's device must remain distinct from A's"
        );

        // The transferred device list (A's register) plus B's own
        // re-registration: B lists both devices under the shared identity.
        let devs_a = a.cli_ok(&["devices"]);
        assert!(
            devs_a.contains(&device_a),
            "A's device list must include its own device: {devs_a}"
        );
        let devs_b2 = b2.cli_ok(&["devices"]);
        assert!(
            devs_b2.contains(&device_a) && devs_b2.contains(&device_b_after),
            "B must list both A's and its own device: {devs_b2}"
        );
        b2.stop();
    }

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

/// Phase 4 of multi-device identity: sync keeps paired devices in step.
///
/// Scenario:
///   1. A and B start as two fresh devices; B pairs onto A (Phase 3 flow).
///   2. B restarts and adopts A's identity. Both now share one identity.
///   3. A sets its profile (`canopee profile --name Alice`), publishing a new
///      signed pointer to the DHT.
///   4. B syncs (`canopee sync`): the DHT pointer is newer than B's (empty)
///      cache, so B fetches the profile object and refreshes its cache.
///   5. B loads the profile and sees Alice's display name.
#[test]
fn two_nodes_sync_profile() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_sync_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. Pair B onto A.
        let out = b.cli_ok(&["pair"]);
        let code = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("Pairing code: ").map(str::to_string))
            .unwrap_or_else(|| panic!("no pairing code in: {out}"));
        let payload = out
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("canopee pair ")
                    .map(|s| s.split(" --code").next().unwrap().to_string())
            })
            .unwrap_or_else(|| panic!("no qr payload in: {out}"));
        a.cli_ok(&["pair", &payload, "--code", &code]);

        // 2. Restart B to adopt A's identity.
        b.stop();
        let mut b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        assert_eq!(
            b2.identity_id(),
            a.identity_id(),
            "B must adopt A's identity after pairing"
        );

        // 3. A sets its profile (after B has adopted the identity, so B
        //    doesn't have it yet).
        a.cli_ok(&["profile", "--name", "Alice"]);

        // 4. B syncs until it sees the profile (DHT propagation is async).
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            let out = b2.cli_ok(&["sync"]);
            if out.contains("profile updated") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B never saw A's profile after sync"
            );
            std::thread::sleep(Duration::from_secs(2));
        }

        // 5. B loads the profile and sees Alice's display name.
        let profile = b2.cli_ok(&["profile"]);
        assert!(
            profile.contains("Alice"),
            "B's profile must be Alice's: {profile}"
        );

        b2.stop();
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

/// Phase 4.3: periodic background sync keeps paired devices in step without
/// any manual `canopee sync` call.
///
/// Scenario:
///   1. A and B start as two fresh devices; B pairs onto A (Phase 3 flow).
///   2. B restarts and adopts A's identity.
///   3. A sets its profile (`canopee profile --name Alice`).
///   4. B does NOT manually sync — the runtime's periodic sync task (30s
///      interval, only when online + connected) fires and refreshes the
///      profile from the DHT.
///   5. B loads the profile and sees Alice's display name.
#[test]
fn two_nodes_periodic_sync() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_periodic_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. Pair B onto A.
        let out = b.cli_ok(&["pair"]);
        let code = out
            .lines()
            .find_map(|l| l.trim().strip_prefix("Pairing code: ").map(str::to_string))
            .unwrap_or_else(|| panic!("no pairing code in: {out}"));
        let payload = out
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("canopee pair ")
                    .map(|s| s.split(" --code").next().unwrap().to_string())
            })
            .unwrap_or_else(|| panic!("no qr payload in: {out}"));
        a.cli_ok(&["pair", &payload, "--code", &code]);

        // 2. Restart B to adopt A's identity.
        b.stop();
        let mut b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        assert_eq!(
            b2.identity_id(),
            a.identity_id(),
            "B must adopt A's identity after pairing"
        );

        // 3. A sets its profile.
        a.cli_ok(&["profile", "--name", "Alice"]);

        // 4. B does NOT manually sync — wait for the periodic sync task to
        //    fire (30s interval, first tick skipped while not yet connected).
        //    IMPORTANT: do NOT poll `canopee profile` here — `load_profile`
        //    populates B's local cache from the DHT via `resolve_pointer`,
        //    which would make the periodic sync see the cache as up-to-date
        //    and skip the update. Wait a fixed time, then check once.
        std::thread::sleep(Duration::from_secs(45));

        // 5. B loads the profile and sees Alice's display name (proof the
        //    periodic sync fired and imported it).
        let profile = b2.cli_ok(&["profile"]);
        assert!(
            profile.contains("Alice"),
            "B never saw A's profile via periodic sync: {profile}"
        );

        b2.stop();
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
