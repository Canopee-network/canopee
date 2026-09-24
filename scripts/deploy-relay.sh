#!/usr/bin/env bash
#
# Canopee relay deploy — build, install and verify the public bootstrap/relay
# node on a VPS. Idempotent and reproducible: run it again (or from CI) to
# roll out an update. See deploy/README.md for the full manual steps this
# script automates.
#
# Usage:
#   scripts/deploy-relay.sh [--targets <list>] [--skip-build]
#
# Options:
#   --targets native,linux-x86_64,linux-arm64  restrict which targets to build
#                                              (default: all available locally)
#   --skip-build                               use existing _binaries artifacts
#
# Environment (all optional):
#   RELAY_HOST        VPS ip/host          (default 89.127.234.35)
#   RELAY_SSH_PORT    ssh port             (default 22)
#   RELAY_SSH_USER    ssh user             (default root)
#   RELAY_SSH_KEY     ssh private key path (default ~/.ssh/1984-root)
#   RELAY_USER        service account on the box (default canopee)
#   RELAY_DIR         install dir on the box      (default /home/canopee/canopee)
#   RELAY_LISTEN_PORT fixed libp2p port           (default 4001)
#   RELAY_MIN_FREE_GB abort deploy below this much free disk (default 1)
#   CANOPEE_BINARIES_ROOT  local _binaries root    (default ../_binaries next to this repo)
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARIES_ROOT="${CANOPEE_BINARIES_ROOT:-$(cd "$REPO_ROOT/.." && pwd)/_binaries}"

RELAY_HOST="${RELAY_HOST:-89.127.234.35}"
RELAY_SSH_PORT="${RELAY_SSH_PORT:-22}"
RELAY_SSH_USER="${RELAY_SSH_USER:-root}"
RELAY_SSH_KEY="${RELAY_SSH_KEY:-$HOME/.ssh/1984-root}"
RELAY_USER="${RELAY_USER:-canopee}"
RELAY_DIR="${RELAY_DIR:-/home/canopee/canopee}"
RELAY_LISTEN_PORT="${RELAY_LISTEN_PORT:-4001}"
RELAY_MIN_FREE_GB="${RELAY_MIN_FREE_GB:-1}"
RELAY_BIN_DIR="$RELAY_DIR/target/release"

SSH=(ssh -i "$RELAY_SSH_KEY" -p "$RELAY_SSH_PORT" -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new)
SCP=(scp -i "$RELAY_SSH_KEY" -P "$RELAY_SSH_PORT" -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new)
REMOTE="$RELAY_SSH_USER@$RELAY_HOST"

log()  { printf '\033[1;34m[deploy]\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m[ ok   ]\033[0m %s\n' "$*"; }
fail() { printf '\033[1;31m[ FAIL ]\033[0m %s\n' "$*" >&2; }
die()  { fail "$*"; exit 1; }

ssh_run() { "${SSH[@]}" "$REMOTE" "$@"; }
scp_to()  { "${SCP[@]}" "$1" "$REMOTE:$2"; }

# --- argument parsing -------------------------------------------------------
BUILD_TARGETS=""
SKIP_BUILD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --targets)  BUILD_TARGETS="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --help|-h)  grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown option: $1 (see --help)" ;;
  esac
done

# rust target  -> _binaries directory name
declare -A FOLDER=( [aarch64-apple-darwin]="macOS-arm64" [x86_64-unknown-linux-musl]="linux-x86_64" [aarch64-unknown-linux-musl]="linux-arm64" )

is_darwin() { [[ "$(uname -s)" == "Darwin" ]]; }

# --- build ------------------------------------------------------------------
resolve_linker() { # $1 = rust target; finds a usable musl gcc and exports CARGO_TARGET_*_LINKER
  local rust_target="$1" upname base
  upname="${rust_target//-/_}"
  upname="$(printf '%s' "$upname" | tr '[:lower:]' '[:upper:]')"
  if [[ "$rust_target" == *"linux-musl" ]]; then
    case "$rust_target" in
      x86_64-*)   base="x86_64-linux-musl-gcc" ;;
      aarch64-*)  base="aarch64-linux-musl-gcc" ;;
      *)          return 0 ;;
    esac
    local linker
    if command -v "$base" >/dev/null 2>&1; then
      linker="$base"
    elif command -v "musl-gcc" >/dev/null 2>&1; then
      linker="musl-gcc"
      log "using musl-gcc for $rust_target (override the .cargo/config.toml linker)"
    else
      die "no linker for $rust_target: install $base (or musl-gcc) or see deploy/README.md"
    fi
    export "CARGO_TARGET_${upname}_LINKER=$linker"
    ok "linker for ${rust_target}: ${linker}"
  fi
}

build_target() { # $1 = rust target
  local rust_target="$1" folder="${FOLDER[$1]}"
  [[ -n "$folder" ]] || die "unknown target: $rust_target"
  resolve_linker "$rust_target"
  log "building $rust_target (node + cli)…"
  cargo build --release --target "$rust_target" -p canopee-node -p canopee-cli
  local node_dir="$BINARIES_ROOT/canopee-node_binaries/$folder"
  local cli_dir="$BINARIES_ROOT/canopee-cli_binaries/$folder"
  mkdir -p "$node_dir" "$cli_dir"
  cp "target/$rust_target/release/canopee-node" "$node_dir/canopee-node"
  cp "target/$rust_target/release/canopee-cli"  "$cli_dir/canopee-cli"
  chmod +x "$node_dir/canopee-node" "$cli_dir/canopee-cli"
  ok "$folder: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
}

cd "$REPO_ROOT"

if [[ $SKIP_BUILD -eq 0 ]]; then
  if [[ -z "$BUILD_TARGETS" ]]; then
    BUILD_TARGETS="linux-x86_64,linux-arm64"
    is_darwin && BUILD_TARGETS="native,$BUILD_TARGETS"
  fi
  IFS=',' read -ra targets <<< "$BUILD_TARGETS"
  for t in "${targets[@]}"; do
    case "$t" in
      native) is_darwin && build_target aarch64-apple-darwin || log "skipping native (not on macOS)" ;;
      linux-x86_64) build_target x86_64-unknown-linux-musl ;;
      linux-arm64)  build_target aarch64-unknown-linux-musl ;;
      *) die "unknown --targets entry: $t" ;;
    esac
  done
else
  log "--skip-build: using existing artifacts under $BINARIES_ROOT"
fi

# --- remote checks ----------------------------------------------------------
log "connecting to $REMOTE …"
ssh_run true >/dev/null || die "cannot ssh to $REMOTE"

local_sha="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
free_kb="$(ssh_run 'df -P / | awk "NR==2{print \$4}"')"
free_gb="$(( free_kb / 1024 / 1024 ))"
log "remote: $(ssh_run 'lsb_release -ds 2>/dev/null || uname -sr')"
log "remote free disk: ${free_gb} GB"
if (( free_gb < RELAY_MIN_FREE_GB )); then
  die "only ${free_gb} GB free on the box; need >= ${RELAY_MIN_FREE_GB} GB (see deploy/README.md cleanup notes)"
fi

remote_arch="$(ssh_run uname -m)"
case "$remote_arch" in
  x86_64)  remote_folder="linux-x86_64" ;;
  aarch64|arm64) remote_folder="linux-arm64" ;;
  *) die "unsupported remote arch: $remote_arch" ;;
esac
[[ -f "$BINARIES_ROOT/canopee-node_binaries/$remote_folder/canopee-node" ]] \
  || die "no built artifacts for $remote_folder under $BINARIES_ROOT"

# --- upload ------------------------------------------------------------
stage="/tmp/canopee-deploy.$$"
ssh_run "rm -rf '$stage' && mkdir -p '$stage'"
BIN_NODE="$BINARIES_ROOT/canopee-node_binaries/$remote_folder/canopee-node"
BIN_CLI="$BINARIES_ROOT/canopee-cli_binaries/$remote_folder/canopee-cli"
log "uploading $remote_folder node+cli…"
scp_to "$BIN_NODE" "$stage/canopee-node"
scp_to "$BIN_CLI"  "$stage/canopee-cli"
ssh_run "
set -euo pipefail
install -d -o '$RELAY_USER' -g '$RELAY_USER' '$RELAY_BIN_DIR'
cp '$stage/canopee-node' '$stage/canopee-node.new'
cp '$stage/canopee-cli'  '$stage/canopee-cli.new'
chown '$RELAY_USER:$RELAY_USER' '$stage/canopee-node.new' '$stage/canopee-cli.new'
mv -f '$stage/canopee-node.new' '$RELAY_BIN_DIR/canopee-node'
mv -f '$stage/canopee-cli.new'  '$RELAY_BIN_DIR/canopee-cli'
chmod 755 '$RELAY_BIN_DIR/canopee-node' '$RELAY_BIN_DIR/canopee-cli'
ls -la '$RELAY_BIN_DIR/canopee-node' '$RELAY_BIN_DIR/canopee-cli'
"

log "installing systemd unit…"
scp_to "$REPO_ROOT/deploy/canopee-node.service" "$stage/canopee-node.service"
ssh_run "
set -euo pipefail
cp '$stage/canopee-node.service' /etc/systemd/system/canopee-node.service
chmod 644 /etc/systemd/system/canopee-node.service
systemctl daemon-reload
systemctl enable canopee-node >/dev/null 2>&1 || true
"
ok "unit installed"

# --- restart + verify ------------------------------------------------------
log "restarting canopee-node.service…"
ssh_run "systemctl restart canopee-node"
sleep 2

# wait for the node to accept socket traffic
for _ in $(seq 1 30); do
  if ssh_run "systemctl is-active --quiet canopee-node 2>/dev/null && [ -S '$RELAY_DIR/../.canopee/node.sock' ]" 2>/dev/null; then
    break
  fi
  sleep 1
done

set +e
ssh_run "systemctl is-active canopee-node"
active_ok=$?
ssh_run "ss -ltn | grep -E ':$RELAY_LISTEN_PORT '" | grep -q "LISTEN"
port_ok=$?
ssh_run "curl -skm5 -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/ | grep -q 404"
edge_ok=$?
set -e

echo
echo "=== relay deployment summary ==="
printf '  service active      : '; ssh_run "systemctl is-active canopee-node"
printf '  listening :%-5s    : ' "$RELAY_LISTEN_PORT"; ssh_run "ss -ltn | grep -E ':$RELAY_LISTEN_PORT '" | awk '{print $4}' | head -1
printf '  edge http :8080    : '; ssh_run "curl -skm5 -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/" || printf 'down'
echo
printf '  deployed version    : '; ssh_run "$RELAY_BIN_DIR/canopee-node --version"
printf '  local   version     : canopee-node  (built from %s)\n' "$local_sha"
printf '  identity           : '; ssh_run "cd -P '$RELAY_DIR/..'; '$RELAY_BIN_DIR/canopee-cli' identity 2>/dev/null || sudo -u '$RELAY_USER' '$RELAY_BIN_DIR/canopee-cli' identity"
printf '  device peer id     : '; ssh_run "sudo -u '$RELAY_USER' '$RELAY_BIN_DIR/canopee-cli' device 2>/dev/null" | head -1
echo
echo '  relay multiaddr (share this):'
printf '    /ip4/%s/tcp/%s/p2p/%s\n' "$RELAY_HOST" "$RELAY_LISTEN_PORT" "$(ssh_run "sudo -u '$RELAY_USER' '$RELAY_BIN_DIR/canopee-cli' device" | head -1)"
echo
echo '  point nodes at the relay:'
echo "    CANOPEE_BOOTSTRAP_ADDRS=/ip4/$RELAY_HOST/tcp/$RELAY_LISTEN_PORT/p2p/<peer-id>"
echo "    canopee dial             /ip4/$RELAY_HOST/tcp/$RELAY_LISTEN_PORT/p2p/<peer-id>"
echo

if [[ $active_ok -ne 0 ]]; then die "canopee-node.service is not active"; fi
if [[ $port_ok -ne 0 ]]; then die "port $RELAY_LISTEN_PORT not listening — check journalctl -u canopee-node"; fi
if [[ $edge_ok -ne 0 ]]; then log "warning: edge role not answering on :8080 (expected only if CANOPEE_EDGE=0)"; fi
ok "deploy complete: $RELAY_HOST is live"