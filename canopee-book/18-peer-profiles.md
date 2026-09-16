# Chapter 18: Peer Profiles

## The Profile Record

Canopee has a working profile system. A profile is a signed, versioned record pointed to by the mutable pointer `(owner, "profile")`:

```rust
pub struct Profile {
    pub display_name: String,      // human-readable name
    pub dh_public_key: [u8; 32],   // X25519 key for end-to-end encryption
    pub avatar: Option<ObjectId>,  // optional avatar image (content-addressed blob)
    pub version: u64,              // monotonic version counter
}
```

This is the entire profile: a display name, a Diffie-Hellman public key (so peers can establish encrypted conversations), an optional avatar, and a version number. Everything is signed by the owner's identity, so a profile is verifiably authentic — no one can forge your profile.

## How Profiles Work

### Saving a Profile

```rust
// Runtime (embedded applications)
runtime.save_profile(Profile {
    display_name: "Alice Smith".to_string(),
    dh_public_key: identity.dh_public_key(),
    avatar: Some(avatar_object_id),
    version: 1,
})?;

// SDK (client-server applications)
client.save_profile(profile)?;
```

Under the hood, the node:

1. Wraps the `Profile` in an `ObjectPayload` with the owner's identity
2. Computes the content-addressed `ObjectId`
3. Signs the payload with the owner's private key
4. Stores the object in `~/.canopee/storage/`
5. Repoints the record `(owner, "profile")` at the new object
6. Caches the record locally (authoritative, instant resolution)
7. Publishes the record to the DHT (background, best-effort)

The version number is incremented server-side — clients send raw profile data, and the node signs and version-bumps it, keeping authorship unspoofable.

### Loading a Profile (Own or Known Owner)

```rust
// Your own profile (local, instant) — load_profile takes no argument
let mine = runtime.load_profile().await?;

// Any known owner (local cache first, then DHT)
let profile = runtime.resolve_profile(&owner).await?;
```

Resolution order:
1. Local `records/` cache (instant)
2. DHT lookup (bounded at 10 seconds)
3. Signature verification (reject if invalid)

### Resolution Semantics

```rust
// resolve_pointer returns the record envelope (an AppPointerRecord), not
// the profile. Fetch the referenced object, then decode it.
if let Some(record) = runtime.resolve_pointer(&owner, "profile").await? {
    let profile_obj = runtime.fetch_object(record.manifest.clone(), None).await?;
    let profile: Profile = profile_obj.decode()?;
}
```

Because records can arrive from arbitrary DHT peers, resolution **always** verifies:
1. The record's Ed25519 signature
2. That the embedded public key derives the claimed owner's `PeerId`

A forged or tampered profile is rejected and resolution returns `None`.

## The Profile Lifecycle

```
Alice publishes profile (version 1)  →  local copy + DHT record
                                            ↓
Bob (knows Alice's identity)  →  resolve_pointer(alice, "profile")
                                        →  verify signature →  Profile
```

- **Creation**: first `save_profile` creates the profile, version 1
- **Update**: subsequent `save_profile` calls write a new object and repoint the record
- **Discovery**: anyone who knows Alice's `IdentityId` can resolve her current profile
- **Integrity**: every read and write goes through the verification pipeline

## Peer Enrichment

The most common way profiles surface is through **peer enrichment**. When you list connected peers, Canopee enriches each one with its human-readable metadata:

```rust
runtime.enrich_peers().await?
```

For each connected peer whose identity is known, the runtime concurrently resolves:
- Their **username** from `(owner, "username")`
- Their **display_name** from `(owner, "profile")`

The result is a friendly peer list:

```rust
Peer 12D3KooWBOB... {
    identity: Some(canopee://identity/12D3KooWBOB...),
    username: Some("bob"),
    display_name: Some("Bob Jones"),
}
```

This is what powers `canopee peers` and the chat app's contact list. Both resolutions have a 5-second timeout so a slow DHT never blocks the peer list.

## The Username Complement

Profiles carry *presentation* (a display name); the username system carries *addressing* (a stable alias). They complement each other:

| System | Purpose | Record |
|--------|---------|--------|
| Username | Stable, claimable alias for addressing | `(owner, "username")` + registry `username:<name>` |
| Profile | Rich, self-described presentation | `(owner, "profile")` |

Usernames are useful for routing: `canopee://alice/<thing>` resolves to `canopee://identity/<peer-id>/<thing>`. Profiles are useful for display: showing a name and an avatar instead of a 52-character peer ID.

## Profile Access Across the Stack

The profile system is wired through every layer:

| Layer | Capability |
|-------|------------|
| **Storage** | `Profile` struct, `(owner, "profile")` pointer |
| **Runtime** | `save_profile`, `load_profile`, `resolve_profile` |
| **Node daemon** | `SaveProfile`, `LoadProfile` commands (node signs) |
| **SDK** | `save_profile`, `load_profile` |
| **Gateway** | `saveProfile`, `loadProfile` + browser-safe `ProfileView` |
| **CLI** | Display names via `peers` (no direct profile command) |

## Retrieving Avatars

The avatar is an `ObjectId` — a pointer to a content-addressed `Blob` object, not an embedded image. To render a profile:

```rust
if let Some(profile) = runtime.resolve_profile(&owner).await? {
    if let Some(avatar_id) = profile.avatar {
        // network is a public field; get_object takes (PeerId, ObjectId) by
        // value. `owner` is an IdentityId — convert it to a PeerId first.
        let bundle = runtime.network
            .get_object(owner_peer_id, avatar_id)
            .await?;
        // avatar bytes are in bundle.object.payload.data
    }
}
```

Avatars can be cached locally (the object store) and are verified on every read.

## What's Missing Today

The current profile system is functional but deliberately minimal. The following do **not** exist yet:

### Richer Profile Fields

There is no bio, no interests, no tags, no location, no organization, no status — beyond `display_name`, `dh_public_key`, `avatar`, and `version`. The `Profile` struct has exactly those four fields.

### Profile Browsing / Directory

There is no way to browse profiles of peers you don't already know about. `enrich_peers()` only resolves metadata for peers you are *already connected to*. There is no DHT-wide profile discovery, no peer directory, no profile gallery.

### Profile Search

There is no way to find a profile by name, interest, or content. The core exposes no search — but [Chapter 19: Semantic Search](19-semantic-search.md) documents a *working* application-layer design (interest discovery) that finds peers by interest-phrase, and the generic gap it leaves.

### Profile in the Chat App

The chat test application does not let users save, view, or edit a profile. It uses identities, usernames, and contacts — but not the `Profile` record. A reference implementation of the full profile lifecycle would be a valuable addition.

## The Worked Example: Interest Discovery

One richer-profile pattern is already *implemented and working* in the sibling project `interest-discovery`. It doesn't extend the core `Profile` struct — instead it demonstrates the idiomatic way to add profile richness in Canopee: **publish your own record shape** on top of the primitives.

```rust
pub struct InterestProfile {
    pub display_name: String,
    pub interests: Vec<String>,   // e.g. "music production", "guitar"
    pub version: u64,
    pub updated_at: u64,
}
```

This is stored as a shared `Blob` object, pointed at by an app-specific record `(owner, "interest-discovery")`, so:

- Any peer can resolve it from a bare peer ID (`resolve_pointer` → `fetch_object`)
- It is fetchable and served because it went through `share_object`
- Its signature is verified by the node on import — it is authentic by construction

Peers who list interests become *discoverable*: the app announces their interest buckets as DHT provider records (see [Chapter 19: Semantic Search](19-semantic-search.md)), so other users can ask "who does music production?" and get back matching peers — with the profile name and the matching interests.

The lesson generalizes: when the four core profile fields aren't enough, the application defines its own signed, shared, resolvable profile object. The `InterestProfile` is the canonical worked example.

## Design Sketches (Future Work)

The following are *proposals*, not implemented features. They are sketched to show how richer profiles would fit Canopee's object model.

### Backward-Compatible Rich Profiles

The `Profile` struct can evolve without breaking existing readers, because records are versioned and readers decode what they understand:

```rust
// Proposed: extended profile, version 2
// Existing readers still decode display_name/dh_public_key/avatar and ignore the rest.
pub struct ExtendedProfile {
    pub display_name: String,
    pub dh_public_key: [u8; 32],
    pub avatar: Option<ObjectId>,
    pub version: u64,
    // Proposed new fields:
    pub bio: Option<String>,
    pub tags: Vec<String>,              // e.g. ["rust", "photography", "gamedev"]
    pub location: Option<String>,
    pub website: Option<String>,
    pub status: Option<String>,          // e.g. "available", "away"
}
```

A new object type (`ProfileV2`) or a superset struct keeps old clients working while new clients surface more.

### Portable Profiles via ExportBundle

Profiles already travel as `ExportBundle` — the entire profile (signature + public key + content hash) can be imported by any peer. A "business card" flow is just: export your profile, send it as an attachment (e.g., through the encrypted chat channel in Chapter 14), and the recipient imports and verifies it.

### Profile Discovery Through the HomeIndex

Because profile objects are entrypoint-able through the `HomeIndex`, a natural (and privacy-preserving) discovery mechanism would combine:
- `load_home_index(owner)` to see what a user chooses to expose
- `resolve_profile(owner)` to fetch the profile itself
- Only objects marked `shared: true` are served — discovery respects the opt-in gate

### The Honest Summary

**Basic profiles are implemented; rich profiles, browsing, and generic search are not.** What exists and what doesn't:

- `Profile` (core) — implemented and wired through every layer
- Rich profile *data* — achievable today by publishing app-specific profile records (the `InterestProfile` pattern is the working proof)
- Interest-based discovery — implemented end-to-end by `interest-discovery` (see next chapter)
- Generic profile browsing (a directory of everyone) and full-text search — still open

Profiles remain among the most valuable objects to index, since they're shared, signed, and describe their owners.