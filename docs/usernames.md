# Usernames: friendly, global peer names

A **username** is a globally unique, human-friendly name (e.g. `alice`) that
resolves to a Canopee identity — so peers can discover, display, and address
you by name instead of a raw libp2p peer id (`12D3KooW…`).

This builds on the user-record layer: a username is a signed
`(owner, "username")` pointer, exactly like `profile` / `contacts` / `home`,
plus a **global DHT registry record** so anyone can reverse-resolve the name
without knowing your peer id up front.

## How it works

Two records are published when you claim a name:

1. **The signed username record** — `UsernameRecord { username, version }`
   stored as a signed object and pointed to by `(owner, "username")`, via the
   same cache-aware pointer layer as profiles. The object is *announced* (the
   one thing that is shared-by-design — claiming a public name is the explicit
   "serve this to the network" act) so any peer that resolves the pointer can
   fetch and verify it.
2. **The registry record** — a mutable DHT record
   `username:<lowercased-name>` → your canonical owner
   (`canopee://identity/<peer-id>`). This is what makes reverse lookup
   (name → identity) possible network-wide.

Resolution is **spoof-verified**: `resolve_username` looks up the registry
record, then *requires* the returned owner to publish a matching signed
`(owner, "username")` record. A peer that stuffs someone else's name into the
registry can't produce the matching signed record, so the lookup fails closed.

Usernames are normalized: trimmed, lowercased, and restricted to
`[a-z0-9._-]`.

## CLI

```bash
canopee username claim alice     # claim "alice" for your identity
canopee username show            # your currently claimed username
canopee username lookup alice    # alice -> canopee://identity/12D3KooW...
```

Once claimed, other commands accept the name anywhere a peer id works:

```bash
canopee fetch alice <object-id>  # instead of canopee fetch 12D3KooW... <id>
canopee peers                    # shows "alice" (+ Username: alice)
canopee chat <topic>             # messages render as "alice: hello"
```

`canopee peers` and the chat view fall back to a shortened peer id
(`12D3KooW…abcd`) for peers that haven't claimed a name. Peer display also
prefers a peer's **profile display name** (`Profile.display_name`) when one
is published, then the username, then the shortened peer id.

## Usernames vs. local aliases

The `canopee://` URI scheme's short names (`canopee alias set alice ...`) are
**local-only conveniences** in `~/.canopee/aliases` — they mean something only
on your machine. Usernames are **global**: claimed on the DHT, verifiable by
anyone, and the same name resolves to the same identity for every peer.

The two compose: after claiming/looking up a username you can still
`canopee alias set` it for URI use. (Auto-seeding aliases from the registry
is a deliberate future step — see below.)

## SDK / protocol

`CanopeeClient` exposes:

- `claim_username(name)` — claim/re-claim the node's username.
- `show_username()` — the node's currently claimed username, if any.
- `resolve_username(name)` — reverse-resolve to an `IdentityId` (verified).

`PeerInfo` (from `peers()`) now carries `username` and `display_name` next to
`peer_id` / `identity` / `addresses`, populated opportunistically by the node
when the records are reachable (bounded, concurrent, per-peer timeouts so
`canopee peers` stays snappy).

## Limitations

- **Availability is DHT-bound.** A username resolves only while the owner (or
  a peer holding the records) is reachable and the records are live in the
  DHT. The registry record and provider records are re-announced each session.
- **First-come, best-effort uniqueness.** The DHT is eventually consistent;
  two nodes claiming the same name concurrently can briefly both believe they
  hold it. The signed-record verification guarantees only that the *resolved
  owner* truly published the claim — not that claims are serialized. (A future
  improvement could compare record timestamps to arbitrate.)
