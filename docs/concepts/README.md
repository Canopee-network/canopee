# Concepts

How Canopee works, in digestible pieces. Each page stands alone; read them in
order if you're new.

- [Architecture](architecture.md) — nodes, identities, objects, and the crate
  map that ties them together.
- [Identity](identity.md) — one identity per person, many devices, and how a
  device proves *who* and *where* it is.
- [Objects & pointers](objects.md) — the content-addressed object model and
  the signed, mutable pointers that resolve `(owner, name)` to an object.
- [Networking](networking.md) — discovery, transport, NAT traversal, and
  pub/sub over libp2p.
- [Sharing](sharing.md) — "nothing is shared-by-default" and how objects move
  between peers.
- [Capabilities](capabilities.md) — signed, verifiable grants of
  permission over a resource.
- [Security](security.md) — what is handled, and what is deliberately left to
  the app layer.

---

The practical side of the same material lives in [`../guides/`](../guides/):
hands-on, command-by-command walkthroughs. The exhaustive, machine-checkable
detail lives in [`../reference/`](../reference/).