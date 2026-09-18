# Protocols & DHT records

The wire-level shape of Canopee. Two protocol families exist:

1. **`canopee-protocol`** — the client ⇄ node socket protocol (JSON on the
   Unix socket).
2. **libp2p behaviours** — node ⇄ node (mDNS, Kademlia, gossipsub,
   request-response, relay/DCUtR/autonat, and the two custom application
   protocols).

## 1. Client ⇄ node (`canopee-protocol`)

The daemon listens on `node.sock` (see [Filesystem](filesystem.md)). Messages
are `NodeCommand` (from the client) / `NodeResponse` (from the node), both
`serde` (de)serializable. Every true client protocol message is
request/response except `Subscribe`, which hijacks its stream and keeps
pushing `PubSubMessage` frames.

### Commands (summary)

| Group | Commands |
|---|---|
| Objects | `Put`, `PutObject`, `Get`, `List`, `SetName`, `Export`, `Import` |
| Status | `Status`, `Identity`, `Shutdown` |
| Network | `Dial`, `ListenViaRelay`, `InternetPeers` (`Peers`), `RelayReservations`, `FindProviders`, `FetchObject`, `Announce` |
| Apps | `PublishAppPointer`, `ResolveAppPointer` |
| User records | `PublishPointer`, `ResolvePointer`, `SaveProfile`, `LoadProfile`, `SaveContactList`, `LoadContactList`, `SaveHomeIndex`, `LoadHomeIndex`, `SetHomeEntryShared`, `ShareObject` |
| Usernames | `ClaimUsername`, `ResolveUsername`, `ShowUsername` |
| Identity | `ExportIdentity`, `ImportIdentity`, `Device`, `DeviceList`, `ResolveOwnerDevice`, `AddDevice`, `RemoveDevice` |
| Pairing | `InitiatePairing`, `CompletePairing` |
| Sync | `SyncFromPeer`, `SyncDeviceList` |
| Capabilities | `GrantCapability`, `ListCapabilities`, `RevokeCapability`, `CheckCapability`, `CheckAccess` |

Key semantics:

- `Put` stores object + optional name; the id returned (`ObjectCreated`) is
  content-derived.
- `Subscribe` is a **hijacked stream** — the node pushes `PubSubMessage`
  frames until the client disconnects. It is not a request/response.
- `PublishPointer` / `ResolvePointer` go through the runtime's cache-aware
  layer (cache → DHT → verify → fetch).
- `ShareObject { name, object, app }` upserts a shared home entry and
  announces DHT provider; `SetHomeEntryShared` flips the flag.
- `ImportIdentity { overwrite }` only takes effect after the node restarts.
- `CheckCapability`/`CheckAccess` are the enforcement path used before
  serving a resource (see [Capabilities](../concepts/capabilities.md)).

### Message structs

- `PubSubMessage`, `PeerInfo`, `RelayReservationInfo`, `DeviceInfo`,
  `PairingQrData`, `PairingRecord`, `PairingData`, `PairingPayload`,
  `SyncResult`.

## 2. Node ⇄ node (libp2p in `canopee-network`)

Wired in `behaviour.rs` (`CanopeeBehaviour`):

| Behaviour | Purpose |
|---|---|
| `mdns` | LAN peer discovery (disabled in app-root mode) |
| `kademlia` | DHT: peer discovery, provider records, registries |
| `identify` | protocol/agent negotiation |
| `gossipsub` | pub/sub messaging |
| `request-response` | object fetching between peers |
| `autonat` + `dcutr` | NAT traversal + hole punching |
| `relay` | circuit relay for unreachable peers |

Custom application protocols carried over `request-response`:

| Protocol | Request | Response |
|---|---|---|
| Object exchange | `ObjectRequest { id }` | the object's bytes (or not-found) |
| Pairing | `PairingRequest` | `PairingResponse` |

### DHT mutable records

See [Objects & pointers](../concepts/objects.md) for the full record table:

| Record key | Value | Writer |
|---|---|---|
| `provider:<object-id>` | nodes serving the object | any node that announced |
| `username:<name>` | canonical owner identity | the claiming identity |
| `device:<peer-id>` | canonical owner identity | the device identity |
| `(owner, name)` pointers (via `PublishPointer`/`ResolvePointer`) | `PointerRecord { owner, name, object_id, timestamp, signature }` | the owner |

All can be verified against signatures independent of the swarm.

## Testing

The `canopee-e2e` crate is the canonical protocol testbed: it spawns real
node processes (isolated via `CANOPEE_APP_ROOT`) and exercises the full
`Promise` — mDNS discovery, sharing, fetching, usernames, pairing, sync —
through the real stack. Run with a single-threaded test runner:

```
cargo test -p canopee-e2e -- --test-threads=1
```

## Related

- [Environment variables](environment.md) — bootstrap overrides for private
  networks.
- [Deployment](deployment.md) — running a public relay/bootstrap node.