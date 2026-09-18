# Chatting between peers

`canopee chat <topic>` is a small REPL over gossipsub: everyone subscribed to
the same topic sees everyone else's messages. It's the fastest way to
smoke-test that two nodes are connected and exchanging live pub/sub traffic.
Read [Networking concept](../concepts/networking.md) if terms like mDNS,
relay, or DHT are unfamiliar; this guide focuses on exact commands.

## Two nodes, same machine (isolated)

Each node needs its own identity and socket. `CANOPEE_APP_ROOT` isolates
every path (see [environment reference](../reference/environment.md)):

```bash
# Node A
CANOPEE_APP_ROOT=/tmp/a canopee init
CANOPEE_APP_ROOT=/tmp/a canopee start

# Node B
CANOPEE_APP_ROOT=/tmp/b canopee init
CANOPEE_APP_ROOT=/tmp/b canopee start
```

They discover each other over mDNS on localhost. Confirm:

```bash
CANOPEE_APP_ROOT=/tmp/a canopee peers    # B's peer id should appear
CANOPEE_APP_ROOT=/tmp/b canopee peers    # A's peer id should appear
```

If `peers` is empty after a few seconds, mDNS is blocked (common on some
Wi-Fi/CI) — see [dial and relay](#dial-and-relay) below.

## Chat

Start a terminal chat on any topic (both sides must use the **exact same**
topic string):

```bash
CANOPEE_APP_ROOT=/tmp/a canopee chat general
#   Chatting on topic 'general'. Type a message and press enter to send.
hello from a

CANOPEE_APP_ROOT=/tmp/b canopee chat general
#   hello from a            ← A's message, arriving live
hello from b
```

`publish` does the same thing without an interactive prompt — useful in
scripts:

```bash
CANOPEE_APP_ROOT=/tmp/b canopee publish general "one-shot hello"
```

## Dial and relay

When direct discovery/reachability fails — two nodes on different networks
behind NAT, or mDNS-blocked LANs — connect explicitly.

**Dial** (when you know the peer's address):

```bash
canopee dial /ip4/203.0.113.7/tcp/4001/p2p/12D3KooW...B
```

**Via a relay** (when neither side is publicly reachable): any node can
relay; run one with a fixed port (`CANOPEE_LISTEN_PORT`) — locally as a dry
run, on a public VPS for real use (see
[Deployment](../reference/deployment.md)):

```bash
CANOPEE_APP_ROOT=/tmp/relay CANOPEE_LISTEN_PORT=4001 canopee start
CANOPEE_APP_ROOT=/tmp/relay canopee identity
#   canopee://identity/12D3KooW...Relay
```

Both peers request a circuit reservation to the same relay:

```bash
canopee listen-via-relay /ip4/<relay-ip>/tcp/4001/p2p/12D3KooW...Relay
canopee relay-status     # → Listen address: /ip4/.../p2p-circuit/p2p/<peer-id>
```

Then each dials the other's circuit address (from `relay-status`),
after which hole-punching (dcutr) tries to upgrade the connection to direct
automatically. Once `peers` shows the connection, `chat` works exactly as
above.

## Troubleshooting

| Symptom | Check |
|---|---|
| `peers` empty | Both nodes running? (`canopee status`). mDNS blocked? Try `dial`/relay. |
| `relay-status` shows nothing | Is the relay reachable (`canopee dial <relay-addr>` succeeds)? After `listen-via-relay`, wait a few seconds. |
| `chat`/`publish` fails `InsufficientPeers` | Nodes not actually connected yet — check `peers`; gossipsub needs an established connection before it meshes on a topic. |
| Messages don't arrive | Both sides using the identical topic string? `general` ≠ `General`. |

## Building chat into an app

This guide uses the CLI; the exact same pub/sub primitives are what the
[Tauri chat tutorial](tauri-chat-app.md) puts behind a UI:
`NetworkManager::subscribe(topic)` → stream of frames,
`NetworkManager::publish(topic, data)` → one call to send.