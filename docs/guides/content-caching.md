# Content caching

Fetching an object stores a content-verified copy locally — and from then
on, that copy may be served to future requesters. This is how content
replicates across the network without a central pinning service: **fetching
*is* seeding.**

## How it works

The node maintains an LRU cache index at `~/.canopee/cache.cache` (see
[Filesystem layout](../reference/filesystem.md)) tracking which locally stored
objects were fetched from other peers and when each was last served. Objects
the node's own identity created are never evicted — the cache only touches
content that came from elsewhere.

The `cached_bytes` cap (configurable) bounds total cached storage; when the
cap is exceeded, the least-recently-served fetched objects are evicted first.

## In practice

- A peer that `fetch`es an object from a provider becomes a provider itself
  — `find-providers` returns any peer holding the object, fetched or local.
- An app published via `app-manifest` + `announce` replicates as peers
  `open` or `fetch` it; each fetcher becomes an additional source for future
  requesters (within the cache cap).
- A shared object (`canopee share`): the publisher announces a provider
  record; the first fetcher holds it locally; the second fetcher may be
  served by *either* peer.

## Eviction and safety

- Only fetched objects (not your own `put` objects) are candidates for
  eviction.
- Least-recently-served is evicted first; the node may stop serving a cached
  object when the cache is full, but the provider record in the DHT may
  still point at it (the fetch fails cleanly with a not-found).
- The cache index is a sidecar (`cache.cache`), not critical state — losing
  it simply means the node can't efficiently identify what's cached from
  peers until objects are served again.

## Why this matters

Content replication is organic, not pinning-based. You don't need a
separate infrastructure tier for availability: the more peers fetch an
object, the more sources it has, and the more resilient it is. This is
the same model as BitTorrent's seeders — a fetched object stays around
until explicitly evicted, not until a pinning server stops storing it.