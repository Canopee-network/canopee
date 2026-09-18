# Usernames

A **username** is a globally unique, human-friendly name (e.g. `alice`) that
resolves to a Canopee identity — so peers can discover, display, and address
you by name instead of a raw peer id.

A username is a signed `(owner, "username")` pointer, exactly like profile /
contacts / home, plus a **global DHT registry record** so anyone can
reverse-resolve the name without knowing your peer id up front. The full
model lives in [Identity concept](../concepts/identity.md).

## Claim a username

```bash
canopee username claim alice       # claim "alice" for this node's identity
canopee username show              # show the currently claimed name, if any
```

Usernames are normalized: trimmed, lowercased, restricted to `[a-z0-9._-]`.
They are unique network-wide.

## Resolve a username to an identity

```bash
canopee username lookup alice
#   canopee://identity/12D3KooW...alice-owner
```

Resolution is spoof-verified: `resolve_username` looks up the DHT registry
record `username:<name>`, then **requires** the returned owner to publish a
matching signed `(owner, "username")` record. A peer that stuffs someone
else's name into the registry can't produce the matching signed record, so
the lookup fails closed.

## Use a username anywhere a peer id works

Once a name is claimed, the CLI accepts it wherever a peer id is expected:

```bash
canopee fetch alice <object-id>     # instead of canopee fetch 12D3KooW... <id>
canopee sync alice                  # refresh records from alice
```

`canopee peers` renders names when available: a profile display name
(`Profile.display_name`) is preferred, then the username, then a shortened
peer id.

## SDK

`CanopeeClient` exposes:

- `claim_username(name)` — claim / re-claim the node's username.
- `show_username()` — currently claimed username, if any.
- `resolve_username(name)` — reverse-resolve to a verified `IdentityId`.

`PeerInfo` (from `peers()`) carries `username` and `display_name` populated
opportunistically by the node when the records are reachable (bounded,
concurrent, per-peer timeouts so `canopee peers` stays snappy).

## Usernames vs. local aliases

The `canopee://` URI scheme's short names (`canopee alias set alice …`) are
**local-only conveniences** in `~/.canopee/aliases` — they mean something
only on your machine. Usernames are **global**: claimed on the DHT,
verifiable by anyone, and the same name resolves to the same identity for
every peer.

The two compose: after claiming/looking up a username you can still
`canopee alias set` it for URI use on your own machine.

## Limitations

- **Availability is DHT-bound.** A username resolves only while the owner
  (or a peer holding the records) is reachable and the records are live. The
  registry record and provider records are re-announced each session.
- **First-come, best-effort uniqueness.** The DHT is eventually consistent;
  two nodes claiming the same name concurrently can briefly both believe
  they hold it. The signed-record verification guarantees only that the
  *resolved owner* truly published the claim — not that claims are
  serialized.