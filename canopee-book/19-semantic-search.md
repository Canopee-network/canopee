# Chapter 19: Semantic Search

## The Discovery Gap

Canopee's core retrieval model is built entirely on exact identifiers:

| What you want | How you get it today |
|---------------|---------------------|
| An object | `get(object_id)` — you must know the exact `ObjectId` |
| An object's provider | `find_providers(object_id)` over DHT |
| A record | `resolve_pointer(owner, name)` — you must know owner and name |
| A user's profile | `resolve_profile(owner)` — you must know the `IdentityId` |
| A published app | `resolve_app_pointer(owner, name)` — you must know owner + name |

This is the same situation as IPFS's `ipfs cat <cid>`: content-addressable systems excel at *storing and verifying* content, but they give you **no way to discover** content you don't already have an address for. You cannot ask "who in my network does music production?" or "which of my objects mention Rust?"

Canopee never shipped a search command. But the primitives it exposes — DHT *provider records* plus the SDK — are powerful enough to build similarity search **entirely at the application layer**, with zero help from the core. A standalone project, `interest-discovery`, proves this in working code. This chapter documents the gap, and then that proof.

## The Ideas That Make It Possible

### Idea 1: Deterministic embeddings, computed from the text itself

You can't ask every peer to download a shared machine-learning model — that would be new infrastructure. Instead, use **hashed character n-grams** (fastText-style):

- Split text into tokens (lowercase, alphanumeric-only, ≥3 chars)
- For each token, take all char n-grams of length 3–5
- Each n-gram scatters a ±1 into one of 128 buckets, picked by a stable FNV-1a hash
- L2-normalize the result

Because the geometry is pure integer hashing, **every peer computes the same vector from the same text** — no model download, no pretrained table, no floating-point ambiguity. And because the features are *character* n-grams, similar spelling and shared morphology land closer together: `guitar` and `guitarist` are more similar than `guitar` and `cooking`.

### Idea 2: Locality-sensitive hashing turns "similar" into "same key"

`find_providers` is an *exact* key lookup, so we need similar text to map to the *same* DHT key. That's exactly what LSH (locality-sensitive hashing) provides:

- Generate 8 projections × 8 random hyperplanes each (64 planes total, from a fixed seed — identical everywhere)
- Within each projection, each plane's sign-of-dot-product contributes one bit; 8 bits = one bucket id per projection
- 8 projections = up to 8 bucket keys per *signal token* (deduplicated), so similar text collides in at least one bucket with high probability, and unrelated text almost never does

Bucket keys are `sha256("interest:v<p>:b<id>")` — **virtual object IDs that never back a stored object**.

### Idea 3: A DHT provider record is just "this peer is interested in X"

The key insight: provider records map a key to a set of peers, and they don't require the key to be a *real* object. Publishing a provider record for a virtual bucket key is precisely the statement "I am interested in the topic that lands in this bucket." Searches union `find_providers` over the buckets their query lands in.

This repurposes one of Canopee's oldest primitives (`announce` / `find_providers`) into a decentralized presence and interest plane — with no new infrastructure.

### Idea 4: Verify on the way back, never trust the DHT

Search results are *addresses*, not content. Every candidate found via the DHT is:

1. Resolved: `resolve_pointer(peer, "interest-discovery")` → profile object ID
2. Fetched: `fetch_object(peer, profile_id)` → `ExportBundle`
3. Verified: `import()` — the node checks the SHA-256 content hash and the Ed25519 signature
4. Checked: the object's `owner` really is the peer that advertised it
5. Re-scored: cosine similarity against the peer's **published** interests

A peer can rank its own objects high (harmless), but it can never inject forged content — verification catches it, and the owner check catches impersonation.

## The Reference Implementation: interest-discovery

`interest-discovery` (a sibling project to the Canopee repo) is a standalone Rust app built **only on the `canopee-sdk`** — it touches no node internals, so it works against any running node. Its CLI binary is called `spot`.

### Commands

```
$ spot me
canopee://identity/12D3KooWFYsj...
no interest profile published — run `spot publish`

$ spot publish --name "Alice" --interest "music production" --interest "guitar"
published interest profile: 024e7d20...

$ spot search "music making"
   1.00  Alice  <12D3KooWFYsj...>
       1.00  music production
```

- **`spot publish`** — stores an `InterestProfile { display_name, interests, version, updated_at }` as a shared `Blob` object, repoints the `(owner, "interest-discovery")` pointer at it, and announces the LSH bucket keys of every interest.
- **`spot search <QUERY> [--min-sim N] [--limit N]`** — buckets the query tokens, unions `find_providers`, then enriches and re-scores each candidate as described above. Returns ranked matches with the matching interests.
- **`spot refresh`** — re-announces the last published profile's buckets. DHT provider records are per-session, so after a node restart you'd otherwise become unfindable until the next `publish`.

### The full publish → search flow

```
PUBLISH (peer A)
  InterestProfile → put_object(Blob) → share_object("interest-discovery", pid)
  publish_pointer("interest-discovery", pid)     # (owner, name) record
  for each interest: announce( bucketed LSH keys )   # "I'm interested in X"

SEARCH (peer B)
  for each query token: bucket → LSH keys
  union find_providers(bucket_key) over all keys   → candidate peer ids
  for each candidate (concurrency 8, timeout 20s):
      resolve_pointer(peer, "interest-discovery")      # find the profile
      fetch_object(peer, pid)  → import (node verifies)
      owner == peer?  else skip
      cosine_score(query_tokens × published interests)
  rank ≥ min_sim, cap at limit
```

### Concretely, from the code

Embedding (`embed.rs`) — hashed char n-grams scatter into a fixed vector:

```rust
const DIM: usize = 128;          // small by design — short interest phrases only
const MIN_TOKEN_LEN: usize = 3;

for token in tokens(text) {
    for n in 3..=5 {
        for ngram in char_ngrams(&token, n) {
            let idx = fnv1a(ngram.as_bytes(), SEED_A) % DIM as usize;
            let sign = if fnv1a(ngram.as_bytes(), SEED_B) & 1 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
    }
}  // L2-normalize
```

LSH (`lsh.rs`) — 8 projections × 8 hyperplane bits, planes from a fixed-seed SplitMix64:

```rust
const PROJECTIONS: usize = 8;
const BITS: usize = 8;
const PLANE_SEED: u64 = 0x5EED_C0FF_EE01;   // NEVER changes — shared geometry

fn key_for_bucket(p: usize, id: u16) -> ObjectId {
    ObjectId::new(&hex(Sha256::digest(format!("interest:v{p}:b{id}"))))
}
```

Publish (`discovery.rs`) — share + point + announce:

```rust
let bytes = profile.encode()?;
let pid = client.put_object(bytes, ObjectType::Blob).await?;
client.share_object(POINTER_NAME, pid.clone(), Some(APP_TAG.into())).await?;
client.publish_pointer(POINTER_NAME, pid.clone()).await?;
for interest in &profile.interests {
    for key in buckets_for_text(interest) {
        client.announce(key).await?;   // virtual key, no backing object
    }
}
```

Search (`discovery.rs`) — bucket the query, union providers, then verify + score:

```rust
for key in buckets_for_text(query) {
    for peer in client.find_providers(key.clone()).await? {
        candidates.push(peer);           // dedupe, exclude self, cap at 64
    }
}
// per candidate (parallel, ≤8):
let record = client.resolve_pointer(owner, POINTER_NAME).await?;  // 1
let bundle = client.fetch_object(peer_id, record.manifest).await?; // 2
client.import(bundle).await?;                                     // 3 (verifies)
let object = client.get(pid).await?;
if object.payload.owner != owner { return skip; }                 // 4
let profile = InterestProfile::decode(&object.payload.data)?;
for interest in profile.interests {                               // 5
    best = max over cosine(q, i);  keep if ≥ min_sim
}
```

## Tested, Working, Deterministic

The project ships unit tests proving the geometry behaves:

- `embed("Music production") == embed("music production")` — case-insensitive and deterministic
- `sim("guitar", "guitarist") > sim("guitar", "cooking") + 0.2` — shared morphology ranks higher
- `buckets_for_text("guitar") == buckets_for_text("guitar")` — identical keys for identical text
- related text shares ≥1 bucket; `guitar` vs `cooking` shares **zero**
- tokens shorter than 3 chars are ignored

And a live two-node test (`cargo test --test live_two_nodes`) that runs two real `canopee-node` processes in isolated `$HOME`s, publishes from one, and searches from the other — confirming discovery via mDNS, DHT propagation, signature verification, and the negative case (an unrelated query does *not* match).

## Trade-offs and Boundaries

The reference implementation makes deliberate trade-offs worth documenting:

- **Online-only discovery.** Provider records are ephemeral per node session — you can only find peers who are online *now*. Consistent with "collaborate in the moment," not a global directory.
- **Stale buckets after edits.** There's no `unannounce` exposed to apps, so editing interests leaves old announcements until the node restarts. Harmless — re-scoring filters false candidates and records expire each session.
- **Interests are public.** That's the point of discovery; don't publish what you wouldn't want peers to see.
- **Cost bounds.** 8 projections + a 64-candidate cap + an 8-way concurrency semaphore keep a single query bounded.
- **Embeddings are weak by modern standards** — a deterministic 128-dimensional hashed-character model captures morphology but not synonymy ("music" vs "audio" won't match unless they share n-gram structure). Real transformer embeddings would be an *upgrade*, not a requirement.

## What This Means for Canopee

The most important lesson: **search did not require new core features.** Everything above uses primitives Canopee shipped from day one:

| Primitive | Used for |
|-----------|----------|
| `put_object` + `share_object` | Publishing the profile as a verified, fetchable object |
| `publish_pointer` / `resolve_pointer` | Resolving the profile from a bare peer ID |
| `announce` / `find_providers` | The interest plane (virtual bucket keys as presence) |
| `fetch_object` + `import` | Fetching with node-side signature verification |
| identity/owner checks | Anti-impersonation |

The general lesson generalizes beyond interests: any application that can *describe* its objects in short text (titles, tags, names) can reuse this exact pattern — LSH buckets over deterministic embeddings, provider records as an index plane, verify-on-the-way-back — without touching core Canopee.

## What Core Canopee Still Lacks

The core is still exact-ID by design, and nothing in the workspace ships a `search` command. Gaps that remain at the core level:

- **No local full-text index** of your own objects (metadata + names + content) — `list()` is O(n)
- **No structured profile fields** beyond `display_name`/`dh_public_key`/`avatar` — rich self-description rides on app-level records like `interest-discovery`
- **No `unannounce`** for applications — stale provider records linger until restart

These are *application-layer opportunities* rather than blockers, exactly as this chapter demonstrates.

## Path Forward

```
Idea level:     deterministic embeddings + LSH buckets + provider-record index plane   [DONE — interest-discovery]
App level:      richer profile records, more indexable fields                         [open]
Core level:     local full-text index, structured profile fields, unannounce          [roadmap]
Research tier:  transformer embeddings for synonymy-aware similarity                  [optional]
```

The proof-of-concept tier is complete and tested. The remaining work is about breadth (more indexable data) and depth (stronger embeddings), not about proving the architecture can work.