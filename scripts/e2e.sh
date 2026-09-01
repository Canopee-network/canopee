#!/usr/bin/env bash
#
# End-to-end test: two real `canopee-node` processes (isolated $HOMEs),
# driven through the real `canopee-cli` binary, over the real libp2p stack.
#
# Scenario (same as crates/canopee-e2e/tests/e2e.rs):
#   1. Nodes A and B start and discover each other via mDNS (localhost).
#   2. A stores a file. It is private by default: B's fetch is refused.
#   3. A shares it (`canopee-cli share`): B discovers A as a DHT provider
#      and fetches the object.
#   4. A unshares it (`canopee-cli unshare`): B's fetch is refused again.
#
# Usage:  scripts/e2e.sh
# Cleanup is automatic (trap kills both nodes and removes the temp dir).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
NODE_BIN="$ROOT/target/debug/canopee-node"
CLI_BIN="$ROOT/target/debug/canopee-cli"

SOCKET_TIMEOUT=30
DISCOVERY_TIMEOUT=60
DHT_TIMEOUT=60

fail() { echo "E2E FAILED: $*" >&2; exit 1; }

echo "==> Building canopee-node and canopee-cli"
cargo build -p canopee-node -p canopee-cli || fail "cargo build"

BASE="$(mktemp -d)/canopee-e2e"
mkdir -p "$BASE/a" "$BASE/b"
A_PID=""
B_PID=""

cleanup() {
    [[ -n "$A_PID" ]] && kill "$A_PID" 2>/dev/null || true
    [[ -n "$B_PID" ]] && kill "$B_PID" 2>/dev/null || true
    rm -rf "$BASE"
}
trap cleanup EXIT

start_node() { # <home> <log> ; echoes pid
    HOME="$1" "$NODE_BIN" > "$2" 2>&1 &
    echo $!
}

wait_for_socket() { # <home>
    for _ in $(seq 1 $((SOCKET_TIMEOUT * 10))); do
        [[ -S "$1/.canopee/node.sock" ]] && return 0
        sleep 0.1
    done
    fail "node at $1 never created its socket (see $2)"
}

echo "==> Starting two nodes"
A_PID="$(start_node "$BASE/a" "$BASE/a.log")"
B_PID="$(start_node "$BASE/b" "$BASE/b.log")"
wait_for_socket "$BASE/a" "$BASE/a.log"
wait_for_socket "$BASE/b" "$BASE/b.log"

cli_a() { HOME="$BASE/a" "$CLI_BIN" "$@"; }
cli_b() { HOME="$BASE/b" "$CLI_BIN" "$@"; }

echo "==> Waiting for mDNS discovery"
# Compare against A's actual identity, not just "whatever B sees first"
# (B may also be connected to the public bootstrap relay).
A_ID_LINE="$(cli_a identity)"
PEER_A="$(sed -E 's/.*canopee:\/\/identity\/([A-Za-z0-9]+).*/\1/' <<< "$A_ID_LINE")"
[[ -n "$PEER_A" ]] || fail "could not parse A's peer id from: $A_ID_LINE"
SEEN=""
for _ in $(seq 1 $((DISCOVERY_TIMEOUT * 2))); do
    if cli_b peers | grep -q "$PEER_A"; then
        SEEN=1
        break
    fi
    sleep 0.5
done
[[ -n "$SEEN" ]] || fail "B never discovered A via mDNS (see $BASE/a.log $BASE/b.log)"
echo "    B sees A: $PEER_A"

echo "==> A stores a file"
echo "hello over the p2p wire" > "$BASE/hello.txt"
OBJECT_ID="$(cli_a put "$BASE/hello.txt" | tail -1 | tr -d ' ')"
[[ -n "$OBJECT_ID" ]] || fail "could not parse object id"

echo "==> B fetches the unshared object (must be refused)"
if cli_b fetch "$PEER_A" "$OBJECT_ID" > /dev/null 2>&1; then
    fail "fetching an unshared object unexpectedly succeeded"
fi
echo "    refused, as expected"

echo "==> A shares the file"
cli_a share "hello.txt" "$OBJECT_ID"

echo "==> B discovers A as a DHT provider"
FOUND=""
for _ in $(seq 1 "$DHT_TIMEOUT"); do
    if cli_b find-providers "$OBJECT_ID" | grep -qx "$PEER_A"; then
        FOUND=1
        break
    fi
    sleep 1
done
[[ -n "$FOUND" ]] || fail "B never saw A as a provider of the shared object"

echo "==> B fetches the shared object"
cli_b fetch "$PEER_A" "$OBJECT_ID"
LIST="$(cli_b list)"
grep -q "$OBJECT_ID" <<< "$LIST" || fail "B does not hold the object after fetching"

echo "==> A's home index shows the shared entry"
HOME_OUT="$(cli_a home)"
grep -q "hello.txt" <<< "$HOME_OUT" || fail "home index missing the entry"
grep -q "Shared: yes" <<< "$HOME_OUT" || fail "entry not marked shared"

echo "==> A unshares"
cli_a unshare "hello.txt"
HOME_OUT="$(cli_a home)"
grep -q "Shared: no" <<< "$HOME_OUT" || fail "entry not marked unshared"

echo "==> B fetches after unshare (must be refused)"
if cli_b fetch "$PEER_A" "$OBJECT_ID" > /dev/null 2>&1; then
    fail "fetching an unshared object unexpectedly succeeded"
fi

echo "==> Stopping nodes"
HOME="$BASE/a" "$CLI_BIN" stop > /dev/null 2>&1 || true
HOME="$BASE/b" "$CLI_BIN" stop > /dev/null 2>&1 || true
sleep 1

echo "E2E PASSED"
