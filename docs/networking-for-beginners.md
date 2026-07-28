# Networking & P2P concepts in Canopee — a beginner's guide

This doc explains the networking ideas behind Canopee in plain language, then
walks through actually setting up and connecting nodes. No prior networking
or peer-to-peer (P2P) experience assumed. If you already know what NAT,
relays, and hole punching are, skip to [Setting up Canopee](#setting-up-canopee).

## The problem P2P solves

Normal apps (a website, a mobile app) work like a phone call to a company
switchboard: your device (the **client**) always calls a company's computer
(the **server**), which has a fixed, known address and is always listening
for calls. Your device never needs to be *reachable* — it only ever makes
outgoing calls.

Canopee doesn't have a central server. Every participant runs a **node**,
and any node might need to reach any other node directly — sometimes your
node is the one placing the call, sometimes it's the one that needs to be
reachable. That second part — being reachable, not just calling out — is
where all the complexity below comes from.

## Why "being reachable" is hard: NAT

Almost every device on the internet — laptops, phones, home routers — sits
behind a **NAT** (Network Address Translation). Your home Wi-Fi router has
one public IP address; every device behind it (laptop, phone, smart TV)
shares that one address and gets a private one (like `192.168.1.42`) that
the internet at large can't see or reach directly.

This is normally invisible because of how the phone-call analogy above
works: your laptop calls out to a server, the router remembers "my laptop
started this conversation" and routes the reply back correctly. But if a
*stranger* on the internet tries to call your laptop's address out of the
blue, the router has no idea which device behind it (if any) that call is
for, and drops it.

So: any node that's only reachable through a home/office NAT **cannot be
dialed directly by a stranger**, even though it can freely dial out to
others. This is the central obstacle P2P networking exists to solve. Canopee
solves it with three different techniques, in order of how "in the same
room" the two peers are.

## Technique 1: mDNS — peers on the same local network

If two Canopee nodes are on the same Wi-Fi/LAN (e.g. two laptops in the same
office), there's no NAT problem between them at all — they can talk directly
on the local network. Canopee uses **mDNS** (multicast DNS) so nodes
automatically announce "I'm here, this is my address" to everyone on the
local network, and every node listens for those announcements and connects
automatically.

This is fully automatic — you don't call anything to enable it. If you start
two `canopee-node` processes on the same LAN, they'll typically find and
connect to each other within a second or two, no configuration needed.

> Some restrictive networks (corporate Wi-Fi, some sandboxes/CI environments)
> block the multicast traffic mDNS relies on. If two nodes on the same LAN
> aren't finding each other, mDNS may be blocked — use direct dial (below) as
> a fallback.

## Technique 2: Direct dial — you already know the address

If you already know exactly how to reach a peer — its IP address, port, and
identity — you can just tell your node to connect to it, no discovery
mechanism needed. This works whenever the target is actually reachable:
same LAN, a server with a public IP, or a home connection with the right
port forwarded on the router.

A **multiaddr** is libp2p's address format, e.g.:

```
/ip4/203.0.113.7/tcp/4001/p2p/12D3KooWAbC...
```

Reading it left to right: "over IPv4, at `203.0.113.7`, on TCP port `4001`,
and I expect the peer answering to have identity `12D3KooWAbC...`" (that
last part is a safety check — if someone else answers at that address, the
connection is rejected).

```bash
canopee dial /ip4/203.0.113.7/tcp/4001/p2p/12D3KooWAbC...
```

## Technique 3: The DHT — finding peers by what they have, not where they are

mDNS only works on the local network, and direct dial requires already
knowing an address. For everything else — "I want the data behind object
`X`, and I have no idea who has it or where they are" — Canopee uses a
**DHT** (Distributed Hash Table), specifically an implementation called
**Kademlia**.

Think of the DHT as a shared phone book that no single node owns —
every node holds a small piece of it, and pieces of the same information are
spread across many nodes for redundancy. When a node has an object, it
**announces** itself as a provider for that object's ID in the DHT. Any
other node can later **look up** that object ID and get back a list of peer
IDs that announced it — then dial one of those peers (directly, or via
relay if needed) to actually fetch the data.

```bash
canopee announce <object-id>          # "I have this object" — writes a record to the DHT
canopee find-providers <object-id>    # "who has this?" — reads records from the DHT
```

The DHT solves *discovery* (who has it, and how to reach them) — it doesn't
by itself solve the NAT reachability problem. That's what the next section
covers.

## Technique 4: Relay + hole punching — when neither side is directly reachable

Here's the hard case: two nodes, **both** behind NAT (e.g. two people at
home, neither with a forwarded port), not on the same LAN. Neither can be
dialed directly, and simply knowing "who has the data" from the DHT doesn't
help if you still can't reach them.

The fix borrows an idea from party lines: if neither side can call the
other directly, both sides call a third party that *is* reachable, and it
patches them through.

- A **relay** is any Canopee node that's publicly reachable (has a real,
  dialable address). It doesn't need to be special-purpose software — every
  `canopee-node` already has relay capability built in and turned on. It
  offers to forward traffic for nodes that ask it to.
- A NAT'd node makes a **circuit reservation** with a relay it can reach —
  basically saying "if anyone comes looking for me, forward them here." From
  that point on, other peers can reach it at an address like
  `/ip4/<relay-ip>/tcp/<port>/p2p/<relay-id>/p2p-circuit/p2p/<your-id>`.
- Once a connection is established *through* the relay, both sides
  automatically attempt **hole punching** (the `dcutr` protocol — "Direct
  Connection Upgrade through Relay") to try to upgrade to a direct
  connection, bypassing the relay entirely. This often succeeds even when
  neither side could be dialed cold, because each side punching outward at
  roughly the same time can open a temporary hole in each NAT that the other
  side's outgoing packet slips through. If it fails, traffic just keeps
  flowing through the relay — slower, but it still works.

```bash
canopee listen-via-relay /ip4/<relay-ip>/tcp/<port>/p2p/<relay-peer-id>
```

You only need this when you're not on the same LAN as your peer *and*
neither of you has a directly reachable address. On the same LAN → mDNS
handles it. One side has a public/forwarded address → direct dial handles
it. Only "both sides are hidden behind home routers" needs a relay.

### Where does a relay come from?

There's no special "relay server" software to install — **any** normal
Canopee node that happens to be publicly reachable (a cheap cloud VM with a
public IP, for instance) can act as one, because relay behavior ships in
every node by default. Practically, though, you want a relay to be:

- **Always on** — it only helps if it's actually running when someone needs it.
- **At a stable, known address** — see the caveat below; a normal node's
  listen port changes on every restart, which is fine for an ordinary peer
  but useless as a relay's address if nobody can predict it.

So "setting up a relay" in practice means: rent a small cloud server, run
`canopee-node` on it with a **fixed port** (see next section), open that
port in the firewall, and share its multiaddr with whoever needs it.

> **Current limitation:** out of the box, a node's listen address is
> `/ip4/0.0.0.0/tcp/0` — port `0` means "the OS picks a random free port,"
> which changes every restart. For a relay you want a fixed port so its
> address doesn't change. This isn't exposed as a config option yet; see
> [Setting up a relay](#setting-up-a-relay) below for the workaround.

## Setting up Canopee

### 1. Build it

```bash
cargo build --workspace
```

### 2. Start a node

A **node** is the background process that owns your identity, stores your
data, and participates in the P2P network. You need exactly one running per
machine/user.

```bash
canopee start
# or, without the CLI wrapper:
cargo run -p canopee-node &
```

This creates `~/.canopee/` on first run — your identity keypair, local
object storage, and the Unix socket other tools use to talk to the node. See
[`canopee-config`](../crates/canopee-config/README.md) for the exact layout.

### 3. Talk to it

Everything else — checking status, storing data, connecting to peers — goes
through the `canopee` CLI (a thin wrapper) or an app built on
[`canopee-sdk`](../crates/canopee-sdk/README.md) (the Rust library apps use).
Both talk to the already-running node; neither works if no node is running.

```bash
canopee status                 # is it running? identity? peer count?
canopee identity                # this node's identity
canopee put ./notes.txt         # store a file locally, signed by your identity
canopee list                    # what's stored locally
```

### 4. Connect to another node

Pick the technique from above that matches your situation:

| Your situation | What to do |
|---|---|
| Same Wi-Fi/LAN as the other node | Nothing — mDNS finds them automatically |
| You know their public IP/port | `canopee dial /ip4/.../tcp/.../p2p/...` |
| You don't know where they are, but know what object you want | `canopee find-providers <object-id>`, then dial/fetch from a result |
| Neither of you has a public address | Both `canopee listen-via-relay <relay-address>` against the same relay |

Once connected:

```bash
canopee announce <object-id>              # tell the DHT you have something
canopee find-providers <object-id>        # ask the DHT who has something
canopee publish <topic> "hello"           # broadcast a message to a topic
```

## Setting up a relay

If you and a peer are both behind home NATs and need to reach each other,
one of you (or a neutral third party) needs to run a publicly reachable
node to act as the relay.

1. **Get a small cloud server** with a public IP address (any cheap
   VPS/cloud VM works — DigitalOcean, a free-tier AWS/GCP instance, etc.).
2. **Open a TCP port** in that server's firewall/security group — pick one,
   e.g. `4001`.
3. **Run `canopee-node` on it, bound to that fixed port.** Today the
   listen address is hardcoded to a random port (`tcp/0`), so until that's
   made configurable, the workaround is to edit
   [`crates/canopee-config/src/lib.rs`](../crates/canopee-config/src/lib.rs)'s
   `listen_addr()` to return your chosen port (e.g.
   `"/ip4/0.0.0.0/tcp/4001".to_string()`) before building the relay's binary.
4. **Get its identity and build its full address:**
   ```bash
   canopee identity
   # canopee://identity/12D3KooW...
   ```
   Combine with the server's public IP and the port you opened:
   ```
   /ip4/<server-public-ip>/tcp/4001/p2p/12D3KooW...
   ```
5. **Share that address** with whoever needs to use this relay. Anyone
   behind NAT runs:
   ```bash
   canopee listen-via-relay /ip4/<server-public-ip>/tcp/4001/p2p/12D3KooW...
   ```
   Once both sides of a conversation have a route through the relay, they
   automatically attempt to upgrade to a direct connection in the background
   — nothing further to configure.

No relay-specific software or setup beyond "run a normal node somewhere
public" is required — see [`canopee-network`](../crates/canopee-network/README.md)'s
design notes for why every node ships with relay capability built in.

## Glossary

| Term | Plain-language meaning |
|---|---|
| **Node** | The background Canopee process; owns your identity and data, and participates in the P2P network |
| **Peer** | Any other node you connect to or discover |
| **NAT** | The router behind which most home/office devices hide, which lets you call out but not usually be called |
| **Multiaddr** | libp2p's address format, e.g. `/ip4/1.2.3.4/tcp/4001/p2p/<id>` — where and who |
| **mDNS** | Automatic peer discovery on the same local network, no configuration needed |
| **DHT (Kademlia)** | A shared, no-single-owner lookup table nodes use to find who has a given piece of data |
| **Announce / provider record** | "I have this object" — a note a node leaves in the DHT |
| **Relay** | A publicly reachable node that forwards traffic between two nodes that can't reach each other directly |
| **Circuit reservation** | Asking a specific relay to forward traffic to you |
| **Hole punching (dcutr)** | The automatic attempt to upgrade a relayed connection into a direct one |
| **Gossipsub** | The publish/subscribe messaging system used for topics (`canopee publish`/`subscribe`) |

## Where to go next

- [`testing-chat-between-peers.md`](testing-chat-between-peers.md) — a
  hands-on walkthrough of chatting between two peers, on the same LAN or
  through a relay
- [`canopee-network` README](../crates/canopee-network/README.md) — the
  implementation details behind everything above, for readers comfortable
  with Rust/libp2p
- [`canopee-sdk` README](../crates/canopee-sdk/README.md) — building an app
  against a running node
- [`canopee-cli` README](../crates/canopee-cli/README.md) — every CLI command
- [root README](../README.md) — how all the crates fit together
