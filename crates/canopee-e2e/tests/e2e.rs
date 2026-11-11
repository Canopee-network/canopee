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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Overall guard so a hung node can never hang the test suite forever.
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(240);
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
        Self::spawn_with_env(node_bin, cli_bin, home, &[])
    }

    /// Spawns a node with extra environment variables. The embedded edge role
    /// is disabled unless `CANOPEE_EDGE_HTTP_PORT` is passed explicitly, so
    /// test nodes don't fight over the default HTTP port 8080.
    fn spawn_with_env(
        node_bin: &Path,
        cli_bin: &Path,
        home: &Path,
        extra_env: &[(&str, String)],
    ) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let log = std::fs::File::create(home.join("node.log")).unwrap();
        let log_err = log.try_clone().unwrap();
        let mut cmd = Command::new(node_bin);
        cmd.env("HOME", home)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        if !extra_env
            .iter()
            .any(|(k, _)| *k == "CANOPEE_EDGE_HTTP_PORT")
        {
            cmd.env("CANOPEE_EDGE", "0");
        }
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("failed to spawn canopee-node");

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

/// Spawns `canopee publish <dir>` in the foreground (stdout piped for
/// `wait_for_line`, stderr drained into a shared buffer so a failing
/// publish's error is visible in panic messages instead of deadlocking the
/// child on a full pipe).
struct PublishProc {
    child: Child,
    stdout: std::sync::mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
}

fn spawn_publish(cli: &Path, home: &Path, site: &Path, edge_addr: &str) -> PublishProc {
    let mut child = Command::new(cli)
        .args(["publish", site.to_str().unwrap()])
        .env("HOME", home)
        .env("CANOPEE_EDGE_ADDR", edge_addr)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn `canopee publish`");
    let stdout = spawn_line_reader(child.stdout.take().unwrap());
    let stderr: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    {
        let stderr = stderr.clone();
        let mut pipe = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = pipe.read_to_string(&mut buf);
            *stderr.lock().unwrap() = buf;
        });
    }
    PublishProc {
        child,
        stdout,
        stderr,
    }
}

/// A running `canopee-edge` process: the public HTTP gateway publishers
/// register with. Spawned with piped stdout so the test can parse its
/// advertised listen address (`Edge listens on: ...`).
struct EdgeProc {
    #[allow(dead_code)]
    home: PathBuf,
    child: Child,
    http_port: u16,
    /// The edge's dialable multiaddr (`/ip4/127.0.0.1/tcp/<port>/p2p/<id>`).
    addr: String,
    /// Kept alive for the lifetime of the edge: dropping the receiver ends
    /// the stdout reader thread, which closes the pipe and makes the edge
    /// panic on its next print.
    _stdout: std::sync::mpsc::Receiver<String>,
}

impl EdgeProc {
    fn spawn(edge_bin: &Path, home: &Path) -> Self {
        std::fs::create_dir_all(home).unwrap();
        let listen_port = free_port();
        let http_port = free_port();
        let log = std::fs::File::create(home.join("edge.log")).unwrap();
        let mut child = Command::new(edge_bin)
            .env("HOME", home)
            .env("CANOPEE_EDGE_LISTEN_PORT", listen_port.to_string())
            .env("CANOPEE_EDGE_HTTP_PORT", http_port.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .expect("failed to spawn canopee-edge");
        let rx = spawn_line_reader(child.stdout.take().unwrap());
        let line = wait_for_line(&rx, "Edge listens on:", SOCKET_TIMEOUT, "edge startup");
        let addr = line
            .split("Edge listens on:")
            .nth(1)
            .expect("malformed `Edge listens on` line")
            .trim()
            .replace("/ip4/0.0.0.0/", "/ip4/127.0.0.1/");
        Self {
            home: home.to_path_buf(),
            child,
            http_port,
            addr,
            _stdout: rx,
        }
    }
}

impl Drop for EdgeProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A free TCP port on localhost (bind-0-then-release; small race, fine here).
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Streams a child pipe's lines into a channel so the test can wait for
/// specific output with a deadline.
fn spawn_line_reader(
    out: impl std::io::Read + Send + 'static,
) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(out);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    rx
}

/// Waits for a line from the publish child's stdout containing `needle`.
/// Unlike `wait_for_line`, also watches the child itself: a publish that
/// dies early (its error goes to stderr) panics immediately with that stderr
/// instead of timing out silently.
fn wait_publish_line(proc: &mut PublishProc, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();
    loop {
        if let Ok(Some(status)) = proc.child.try_wait() {
            let stderr = proc.stderr.lock().unwrap().clone();
            panic!(
                "`canopee publish` exited early ({status}) while waiting for `{needle}`; \
                 stdout so far:\n{seen}\nstderr:\n{stderr}"
            );
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for `{needle}` in publish output; saw:\n{seen}\nstderr:\n{}",
            proc.stderr.lock().unwrap()
        );
        match proc
            .stdout
            .recv_timeout(remaining.min(Duration::from_millis(200)))
        {
            Ok(line) => {
                let found = line.contains(needle);
                seen.push_str(&line);
                seen.push('\n');
                if found {
                    return line;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let stderr = proc.stderr.lock().unwrap().clone();
                panic!("publish stdout closed before `{needle}`; saw:\n{seen}\nstderr:\n{stderr}");
            }
        }
    }
}

/// Waits for a line containing `needle`, panicking with everything seen so
/// far on timeout.
fn wait_for_line(
    rx: &std::sync::mpsc::Receiver<String>,
    needle: &str,
    timeout: Duration,
    what: &str,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut seen = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for `{needle}` in {what}; saw:\n{seen}"
        );
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                let found = line.contains(needle);
                seen.push_str(&line);
                seen.push('\n');
                if found {
                    return line;
                }
            }
            Err(_) => panic!("timed out waiting for `{needle}` in {what}; saw:\n{seen}"),
        }
    }
}

/// Sends SIGINT to a process (what Ctrl+C sends to the publish CLI).
fn sigint(pid: u32) {
    let status = Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .expect("failed to run kill");
    assert!(status.success(), "kill -INT {pid} failed");
}

/// `curl` helpers hitting the edge's HTTP port with a specific Host header.
fn curl_status(port: u16, host: &str, path: &str) -> String {
    let out = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "--max-time",
            "20",
            "-H",
            &format!("Host: {host}"),
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .output()
        .expect("failed to run curl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn curl_body(port: u16, host: &str, path: &str) -> String {
    let out = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "20",
            "-H",
            &format!("Host: {host}"),
            &format!("http://127.0.0.1:{port}{path}"),
        ])
        .output()
        .expect("failed to run curl");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf()
}

/// Builds the `canopee-node`, `canopee-cli`, and `canopee-edge` binaries
/// (idempotent — cargo's own fingerprint check makes this cheap on re-runs)
/// and returns the node + CLI binary paths. Note: the CLI binary is
/// `canopee-cli`, not `canopee` — the latter is the workspace root's stub
/// package.
fn build_binaries() -> (PathBuf, PathBuf) {
    let root = workspace_root();
    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "canopee-node",
            "-p",
            "canopee-cli",
            "-p",
            "canopee-edge",
        ])
        .current_dir(&root)
        .status()
        .expect("failed to invoke cargo build");
    assert!(status.success(), "cargo build of e2e binaries failed");
    let debug = root.join("target/debug");
    (debug.join("canopee-node"), debug.join("canopee-cli"))
}

fn edge_binary() -> PathBuf {
    debug_dir().join("canopee-edge")
}

fn debug_dir() -> PathBuf {
    workspace_root().join("target/debug")
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

        // 4. A shares the file under a name, naming B as a recipient.
        //
        // Objects are encrypted to A's own devices, so B needs the content key
        // wrapped for B's key too: A records B as a contact and shares with
        // `--to`. That publishes a per-recipient copy under its own id (objects
        // are addressed by their ciphertext), which is what actually goes on
        // the DHT from here on.
        // B's DH key comes from B's identity directly — it must not depend on
        // B having published a profile.
        let bob_dh = b.cli_ok(&["dh-key"]);
        assert!(
            !bob_dh.trim().is_empty()
                && bob_dh.trim()
                    != "0000000000000000000000000000000000000000000000000000000000000000",
            "B's dh-key must be a real key, got {:?}",
            bob_dh.trim()
        );
        a.cli_ok(&["contact-add", "bob-e2e", &b.device_peer_id(), &bob_dh]);
        a.cli_ok(&["share", "hello.txt", &object_id, "--to", "bob-e2e"]);

        let home_after_share = a.cli_ok(&["home", "--ids"]);
        let shared_id = home_after_share
            .lines()
            .find_map(|l| l.trim().strip_prefix("Object: ").map(|s| s.to_string()))
            .unwrap_or_else(|| panic!("home index exposed no object id: {home_after_share}"));
        assert_ne!(
            shared_id, object_id,
            "a per-recipient copy must get its own id, distinct from A's local original"
        );

        // 5. B discovers A as a provider on the DHT and fetches the object.
        let deadline = Instant::now() + DHT_TIMEOUT;
        let found = loop {
            let (_, out, _) = b.cli(&["find-providers", &shared_id]);
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
        b.cli_ok(&["fetch", "alice-e2e", &shared_id]);
        let list = b.cli_ok(&["list", "--ids"]);
        assert!(
            list.contains(&shared_id),
            "B must hold the object after fetching: {list}"
        );

        // 5a. The point of the whole exercise: B holds ciphertext it was
        // never able to encrypt, and still reads the original bytes back. A
        // different identity, over the DHT, via a relay, without ever holding
        // A's keys.
        let contents = b.cli_ok(&["get", &shared_id]);
        assert_eq!(
            contents.trim(),
            "hello over the p2p wire",
            "B must decrypt the object A shared to it"
        );
        // B's stored copy is genuinely encrypted, not plaintext handed over.
        assert!(
            !contents.contains("CNP1"),
            "the decrypted view must not expose the envelope"
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
            list.contains("hello.txt") && list.contains(&shared_id),
            "the name-fetched object must list by name (and id with --ids): {list}"
        );

        // 6. A's home index shows the shared entry.
        let home = a.cli_ok(&["home"]);
        assert!(
            home.contains("hello.txt"),
            "home must list the entry: {home}"
        );
        assert!(home.contains("Shared: yes"), "entry must be shared: {home}");

        // 7. Unshare: the gate closes again.
        a.cli_ok(&["unshare", "hello.txt"]);
        let home = a.cli_ok(&["home"]);
        assert!(
            home.contains("Shared: no"),
            "entry must be unshared: {home}"
        );

        let (ok, _, _) = b.cli(&["fetch", &identity_a, &shared_id]);
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
    assert_ne!(
        identity_a, identity_b,
        "two fresh nodes have distinct identities"
    );

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
        // The runtime's periodic sync can import the profile between this
        // manual call's cache read and its DHT lookup; the manual call then
        // correctly reports "Already up to date" instead of "profile
        // updated". Treat the profile being visible as success either way.
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            let out = b2.cli_ok(&["sync", "records"]);
            let profile = b2.cli_ok(&["profile"]);
            if out.contains("profile updated") || profile.contains("Alice") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B never saw A's profile after sync (last sync output: {out:?})"
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

/// End-to-end app publishing through an edge: one publisher node + one edge
/// process on localhost, driven through the real `canopee publish` CLI and a
/// real HTTP client (curl).
///
/// Scenario:
///   1. A writes a static site directory (no username needed — apps are
///      keyed by manifest hash).
///   2. `canopee publish <dir>` runs in the foreground: it uploads the site,
///      registers a signed serve claim with the edge, and reports the public
///      URL. The edge validates ownership by fetching the manifest from the
///      publisher and checking its owner.
///   3. An HTTP request to the edge with `Host: <app-hash>.canopee.network`
///      returns the site's index.html, tunneled live from A's node.
///   4. An unregistered subdomain returns 404.
///   5. Ctrl+C (SIGINT) deregisters: the same request then 404s.
#[test]
fn publish_app_over_edge_end_to_end() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();
    let edge_bin = edge_binary();

    let base = std::env::temp_dir().join(format!("canopee_e2e_publish_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_edge = base.join("edge");
    let site = base.join("site");

    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(
        site.join("index.html"),
        "<!doctype html><title>e2e</title><h1>hello from the edge</h1>\n",
    )
    .unwrap();

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut edge = EdgeProc::spawn(&edge_bin, &home_edge);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. `canopee publish` is a foreground command: spawn it and wait for
        //    the manifest id + public URL, which print only after the edge
        //    accepted the serve registration.
        let mut publish = spawn_publish(&cli_bin, &home_a, &site, &edge.addr);
        let manifest_line = wait_publish_line(&mut publish, "Manifest:", DHT_TIMEOUT);
        let manifest_id = manifest_line
            .split("Manifest:")
            .nth(1)
            .expect("malformed Manifest line")
            .trim()
            .to_string();
        let subdomain = &manifest_id[..32];
        let host = format!("{subdomain}.canopee.network");
        let url_line = wait_publish_line(&mut publish, "Public:", DHT_TIMEOUT);
        assert!(
            url_line.contains(&format!("https://{host}/")),
            "unexpected public URL: {url_line}"
        );

        // 2. The app is live through the edge: GET / on the edge's HTTP port
        //    with the app's subdomain returns the site's HTML.
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            let body = curl_body(edge.http_port, &host, "/");
            if body.contains("hello from the edge") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "edge never served the published app; last body: {body:?}"
            );
            std::thread::sleep(Duration::from_millis(500));
        }

        // 3. An unregistered subdomain is not served.
        let status = curl_status(
            edge.http_port,
            &format!("{}.canopee.network", "0".repeat(32)),
            "/",
        );
        assert_eq!(status, "404", "unregistered subdomain must 404");

        // 4. Ctrl+C deregisters: the CLI reports it, and the edge stops
        //    routing the subdomain.
        sigint(publish.child.id());
        wait_publish_line(
            &mut publish,
            "Application taken offline.",
            Duration::from_secs(15),
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match publish.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => {
                    let _ = publish.child.kill();
                    let _ = publish.child.wait();
                    panic!("publish process did not exit after SIGINT");
                }
            }
        }
        let status = curl_status(edge.http_port, &host, "/");
        assert_eq!(status, "404", "subdomain must 404 after deregistration");
    }));

    a.stop();
    edge.child.kill().ok();
    edge.child.wait().ok();
    let _ = std::fs::remove_dir_all(&base);

    assert!(
        started.elapsed() < SCENARIO_TIMEOUT,
        "scenario exceeded its time budget"
    );
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// A plain `canopee-node` doubles as an edge (the default edge role), so
/// `canopee publish` works against any node — this is how the public
/// bootstrap relay serves publishers with no separate gateway process.
///
/// Scenario:
///   1. Node E starts with the edge role on a fixed HTTP + libp2p port.
///   2. Publisher A runs `canopee publish` pointed at E's libp2p address
///      (as if E were the bootstrap relay) — no username, the app is keyed
///      by manifest hash.
///   3. An HTTP request to E with `Host: <app-hash>.canopee.network` returns
///      the site's HTML, tunneled live from A.
///   4. Ctrl+C deregisters and E 404s the subdomain again.
#[test]
fn publish_app_over_embedded_node_edge_end_to_end() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_nodeedge_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_edge = base.join("edge");
    let home_a = base.join("a");
    let site = base.join("site");

    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(
        site.join("index.html"),
        "<!doctype html><title>e2e</title><h1>hello from the node edge</h1>\n",
    )
    .unwrap();

    // Node E: a normal node with the edge role enabled on known ports.
    let edge_http_port = free_port();
    let edge_libp2p_port = free_port();
    let mut edge = TestNode::spawn_with_env(
        &node_bin,
        &cli_bin,
        &home_edge,
        &[
            ("CANOPEE_EDGE_HTTP_PORT", edge_http_port.to_string()),
            ("CANOPEE_LISTEN_PORT", edge_libp2p_port.to_string()),
        ],
    );
    let edge_peer = edge.device_peer_id();
    let edge_addr = format!("/ip4/127.0.0.1/tcp/{edge_libp2p_port}/p2p/{edge_peer}");

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut publish = Command::new(&cli_bin)
            .args(["publish", site.to_str().unwrap()])
            .env("HOME", &home_a)
            .env("CANOPEE_EDGE_ADDR", &edge_addr)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn `canopee publish`");
        let publish_rx = spawn_line_reader(publish.stdout.take().unwrap());
        let manifest_line = wait_for_line(&publish_rx, "Manifest:", DHT_TIMEOUT, "publish output");
        let manifest_id = manifest_line
            .split("Manifest:")
            .nth(1)
            .expect("malformed Manifest line")
            .trim()
            .to_string();
        let subdomain = &manifest_id[..32];
        let host = format!("{subdomain}.canopee.network");
        let url_line = wait_for_line(&publish_rx, "Public:", DHT_TIMEOUT, "publish output");
        assert!(
            url_line.contains(&format!("https://{host}/")),
            "unexpected public URL: {url_line}"
        );

        // The app is live through the node edge.
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            let body = curl_body(edge_http_port, &host, "/");
            if body.contains("hello from the node edge") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "node edge never served the published app; last body: {body:?}"
            );
            std::thread::sleep(Duration::from_millis(500));
        }

        let status = curl_status(
            edge_http_port,
            &format!("{}.canopee.network", "0".repeat(32)),
            "/",
        );
        assert_eq!(status, "404", "unregistered subdomain must 404");

        // Ctrl+C deregisters.
        sigint(publish.id());
        wait_for_line(
            &publish_rx,
            "Application taken offline.",
            Duration::from_secs(15),
            "publish shutdown",
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match publish.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                _ => {
                    let _ = publish.kill();
                    let _ = publish.wait();
                    panic!("publish process did not exit after SIGINT");
                }
            }
        }
        let status = curl_status(edge_http_port, &host, "/");
        assert_eq!(status, "404", "subdomain must 404 after deregistration");
    }));

    a.stop();
    edge.stop();
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

/// Folder sync acceptance test for Canopee Sync v1.
///
/// Scenario:
///   1. A and B pair, and B restarts under A's identity.
///   2. A creates a `docs` sync root with a nested file.
///   3. B joins the root and pulls the initial manifest.
///   4. Both devices converge on the nested file.
///   5. A local write on B triggers a watcher sync without `sync now`.
///   6. B edits a file and creates another; A receives both.
///   7. Deletes propagate in both directions.
///   8. An offline edit on B conflicts with A's edit; one version wins at
///      the original path and the other is preserved as a conflict copy,
///      published in the same sync pass.
///   9. Delete-vs-edit keeps the edit in both directions.
///  10. A deletes while B is stopped; the tombstone survives B's restart.
///  11. Neither device's object store contains plaintext file content.
#[test]
fn two_nodes_sync_folder() {
    let started = Instant::now();
    let (node_bin, cli_bin) = build_binaries();

    let base = std::env::temp_dir().join(format!("canopee_e2e_folder_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home_a = base.join("a");
    let home_b = base.join("b");
    let folder_a = base.join("folder-a");
    let folder_b = base.join("folder-b");

    std::fs::create_dir_all(folder_a.join("sub")).unwrap();
    std::fs::create_dir_all(&folder_b).unwrap();
    let hello = b"hello from canopee sync";
    let large: Vec<u8> = (0usize..(3 * 1024 * 1024))
        .map(|i| (i.wrapping_mul(31).wrapping_add(7) % 251) as u8)
        .collect();
    std::fs::write(folder_a.join("hello.txt"), hello).unwrap();
    std::fs::write(folder_a.join("large.bin"), &large).unwrap();
    std::fs::write(folder_a.join("sub/nested.txt"), b"nested file").unwrap();
    std::fs::write(folder_a.join("delete-local.txt"), b"delete me on B").unwrap();
    std::fs::write(folder_b.join("existing.txt"), b"keep me").unwrap();

    let mut a = TestNode::spawn(&node_bin, &cli_bin, &home_a);
    let mut b = TestNode::spawn(&node_bin, &cli_bin, &home_b);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 1. Pair B onto A, then restart B so it shares A's identity.
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

        b.stop();
        let mut b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        assert_eq!(
            b2.identity_id(),
            a.identity_id(),
            "B must adopt A's identity after pairing"
        );

        // 2. A creates the root; B joins it at a different local path.
        a.cli_ok(&[
            "sync",
            "create",
            folder_a.to_str().unwrap(),
            "--name",
            "docs",
        ]);
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            let (ok, _, stderr) = b2.cli(&["sync", "join", "docs", folder_b.to_str().unwrap()]);
            if ok {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B never resolved A's root index: {stderr}"
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // 3. Pull until the initial tree has arrived.
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            b2.cli_ok(&["sync", "now", "docs"]);
            if file_equals(&folder_b.join("hello.txt"), hello)
                && file_equals(&folder_b.join("large.bin"), &large)
                && file_equals(&folder_b.join("sub/nested.txt"), b"nested file")
                && file_equals(&folder_b.join("delete-local.txt"), b"delete me on B")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B never received A's sync root; destination contains: {:?}",
                list_tree(&folder_b)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // 4. Plaintext must not be present in either encrypted object store.
        for (name, home) in [("A", &home_a), ("B", &home_b)] {
            assert!(
                !dir_contains_bytes(&home.join(".canopee/storage"), hello),
                "{name}'s object store contains plaintext file content"
            );
        }

        // 5. The watcher runs a pass after a local write without `sync now`.
        //    Restart B first so its 30s periodic timer cannot mask a broken
        //    watcher during this short check.
        b2.stop();
        b2 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        b2.wait_for_peer(&a.device_peer_id());
        a.wait_for_peer(&b2.device_peer_id());
        let before = last_sync_millis(&b2, "docs");
        std::fs::write(folder_b.join("watcher.txt"), b"watched").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if last_sync_millis(&b2, "docs") > before {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "watcher did not sync a local write; status: {}",
                b2.cli_ok(&["sync", "status", "docs"])
            );
            std::thread::sleep(Duration::from_millis(200));
        }

        // 6. Edits made on B propagate back to A.
        let from_b = b"edited on B";
        std::fs::write(folder_b.join("hello.txt"), from_b).unwrap();
        std::fs::write(folder_b.join("from-b.txt"), b"created on B").unwrap();

        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            b2.cli_ok(&["sync", "now", "docs"]);
            a.cli_ok(&["sync", "now", "docs"]);
            if file_equals(&folder_a.join("hello.txt"), from_b)
                && file_equals(&folder_a.join("from-b.txt"), b"created on B")
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "A never received B's edits; destination contains: {:?}",
                list_tree(&folder_a)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // Joining must not delete pre-existing local files in v1.
        assert!(
            file_equals(&folder_b.join("existing.txt"), b"keep me"),
            "join must not delete unrelated local files"
        );

        // 7. A local delete on B propagates to A as a tombstone.
        std::fs::remove_file(folder_b.join("delete-local.txt")).unwrap();
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            b2.cli_ok(&["sync", "now", "docs"]);
            a.cli_ok(&["sync", "now", "docs"]);
            if !folder_a.join("delete-local.txt").exists()
                && !folder_b.join("delete-local.txt").exists()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B's delete never reached A; destination contains: {:?}",
                list_tree(&folder_a)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // 8. Stop B and make B's conflicting edits while it is offline. Then
        //    reconnect B before A publishes DHT so the pointer push is
        //    deterministic; B still has not synced since its offline edit.
        b2.stop();
        let from_a = b"edited on A while B was offline";
        let from_b_offline = b"edited on B while A was offline";
        let offline_a = b"created while B was down";
        std::fs::write(folder_b.join("hello.txt"), from_b_offline).unwrap();

        let mut b3 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        assert_eq!(
            b3.identity_id(),
            a.identity_id(),
            "B must keep A's identity after restart"
        );
        b3.wait_for_peer(&a.device_peer_id());
        a.wait_for_peer(&b3.device_peer_id());

        std::fs::write(folder_a.join("hello.txt"), from_a).unwrap();
        std::fs::write(folder_a.join("offline-a.txt"), offline_a).unwrap();
        std::fs::remove_file(folder_a.join("sub/nested.txt")).unwrap();
        a.cli_ok(&["sync", "now", "docs"]);

        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            b3.cli_ok(&["sync", "now", "docs"]);
            if conflict_pair_matches(&folder_b, from_a, from_b_offline)
                && file_equals(&folder_b.join("offline-a.txt"), offline_a)
                && !folder_b.join("sub/nested.txt").exists()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "B did not resolve the conflict; destination contains: {:?}",
                list_tree(&folder_b)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // The conflict copy must be published in the same pass, so one A sync
        // is enough to receive it.
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            a.cli_ok(&["sync", "now", "docs"]);
            if conflict_pair_matches(&folder_a, from_a, from_b_offline)
                && !folder_a.join("sub/nested.txt").exists()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "A did not receive B's conflict copy; destination contains: {:?}",
                list_tree(&folder_a)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        // Extra sync passes must not duplicate conflict files or resurrect
        // the deleted file.
        b3.cli_ok(&["sync", "now", "docs"]);
        a.cli_ok(&["sync", "now", "docs"]);
        b3.cli_ok(&["sync", "now", "docs"]);
        assert_eq!(conflict_files(&folder_a).len(), 1);
        assert_eq!(conflict_files(&folder_b).len(), 1);
        assert!(!folder_a.join("sub/nested.txt").exists());
        assert!(!folder_b.join("sub/nested.txt").exists());

        // 9. Delete-vs-edit keeps the edit in both directions. B makes its
        //    changes while stopped; reconnect it before A publishes.
        b3.stop();
        let remote_edit = b"remote modification wins over local delete";
        let local_edit = b"local modification wins over remote delete";
        std::fs::remove_file(folder_b.join("hello.txt")).unwrap();
        std::fs::write(folder_b.join("offline-a.txt"), local_edit).unwrap();

        let mut b4 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        b4.wait_for_peer(&a.device_peer_id());
        a.wait_for_peer(&b4.device_peer_id());

        std::fs::write(folder_a.join("hello.txt"), remote_edit).unwrap();
        std::fs::remove_file(folder_a.join("offline-a.txt")).unwrap();
        a.cli_ok(&["sync", "now", "docs"]);
        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            b4.cli_ok(&["sync", "now", "docs"]);
            if file_equals(&folder_b.join("hello.txt"), remote_edit)
                && file_equals(&folder_b.join("offline-a.txt"), local_edit)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "delete-vs-edit did not converge on B; destination contains: {:?}",
                list_tree(&folder_b)
            );
            std::thread::sleep(Duration::from_secs(1));
        }

        let deadline = Instant::now() + DHT_TIMEOUT;
        loop {
            a.cli_ok(&["sync", "now", "docs"]);
            if file_equals(&folder_a.join("hello.txt"), remote_edit)
                && file_equals(&folder_a.join("offline-a.txt"), local_edit)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "delete-vs-edit did not converge on A; destination contains: {:?}",
                list_tree(&folder_a)
            );
            std::thread::sleep(Duration::from_secs(1));
        }
        assert_eq!(conflict_files(&folder_a).len(), 1);
        assert_eq!(conflict_files(&folder_b).len(), 1);

        // 10. The tombstone survives a restart: B must not resurrect the file.
        b4.stop();
        let mut b5 = TestNode::spawn(&node_bin, &cli_bin, &home_b);
        b5.cli_ok(&["sync", "now", "docs"]);
        assert!(
            !folder_b.join("sub/nested.txt").exists(),
            "restarted B resurrected a deleted file"
        );
        assert!(!folder_b.join("delete-local.txt").exists());
        assert_eq!(conflict_files(&folder_b).len(), 1);

        // 11. Newly synced plaintext must also stay out of both stores.
        let plaintexts: [&[u8]; 3] = [&from_b_offline[..], &remote_edit[..], &local_edit[..]];
        for content in plaintexts {
            for (name, home) in [("A", &home_a), ("B", &home_b)] {
                assert!(
                    !dir_contains_bytes(&home.join(".canopee/storage"), content),
                    "{name}'s object store contains plaintext sync content"
                );
            }
        }
        let chunk_probe = &large[1_000_000..1_004_096];
        for (name, home) in [("A", &home_a), ("B", &home_b)] {
            assert!(
                !dir_contains_bytes(&home.join(".canopee/storage"), chunk_probe),
                "{name}'s object store contains a plaintext sync chunk"
            );
        }
        b5.stop();
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

fn last_sync_millis(node: &TestNode, root: &str) -> u64 {
    node.cli_ok(&["sync", "status", root])
        .split("last sync ")
        .nth(1)
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

fn conflict_pair_matches(root: &Path, first: &[u8], second: &[u8]) -> bool {
    let conflicts = conflict_files(root);
    if conflicts.len() != 1 {
        return false;
    }
    let hello = root.join("hello.txt");
    (file_equals(&hello, first) && file_equals(&conflicts[0], second))
        || (file_equals(&hello, second) && file_equals(&conflicts[0], first))
}

fn conflict_files(root: &Path) -> Vec<PathBuf> {
    list_tree(root)
        .into_iter()
        .filter(|path| path.contains(".conflict-"))
        .map(|path| root.join(path))
        .filter(|path| path.is_file())
        .collect()
}

fn file_equals(path: &Path, expected: &[u8]) -> bool {
    std::fs::read(path)
        .map(|actual| actual == expected)
        .unwrap_or(false)
}

fn dir_contains_bytes(path: &Path, needle: &[u8]) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            dir_contains_bytes(&path, needle)
        } else {
            std::fs::read(&path)
                .map(|bytes| bytes.windows(needle.len()).any(|window| window == needle))
                .unwrap_or(false)
        }
    })
}

fn list_tree(path: &Path) -> Vec<String> {
    fn visit(root: &Path, dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.display().to_string());
            }
            if path.is_dir() {
                visit(root, &path, out);
            }
        }
    }

    let mut out = Vec::new();
    visit(path, path, &mut out);
    out.sort();
    out
}
