# Chapter 20: User Stories

Each user story demonstrates a specific Canopee feature through a real-world scenario.

---

## Story 1: First-Time Setup

**As a new user, I want to set up Canopee with a single command.**

Alice installs a Canopee-powered application. On first launch, the application calls:

```rust
let config = Config::new()
    .with_app_root(app_data_dir)
    .with_user_root(user_dir)
    .with_mdns(false);

let runtime = Runtime::open_with_config(config).await?;
```

Under the hood, this:
1. Creates `~/.canopee/` with all subdirectories
2. Generates an Ed25519 keypair at `~/.canopee/identity/identity.key`
3. Initializes the object store at `~/.canopee/storage/`
4. Creates the node state at `<app>/state/node.state`

(The `records/` directory is created lazily on the first record publish, not at open.)

Alice now has a cryptographic identity. No username, no password, no email verification. Her identity is `canopee://identity/12D3KooW...`.

She can export her identity key to use on another device:

```bash
cp ~/.canopee/identity/identity.key /usb-drive/identity.key
```

On the other device, she copies the key back and the same identity is restored. No account migration, no data sync — the key is the identity.

---

## Story 2: Storing Personal Data

**As a user, I want to store files locally and control what's shared.**

Alice takes a photo and wants to store it in her Canopee-powered gallery app:

```rust
// Store the photo as a Blob object
let photo_bytes = std::fs::read("sunset.jpg")?;
let photo_id = runtime.put(photo_bytes).await?;  // returns ObjectId
println!("Stored as: {}", photo_id);
// Stored as: a1b2c3d4e5f6...

// Add to home index as private
runtime.save_home_index(HomeIndex {
    profile: None,
    contacts: None,
    entries: vec![HomeEntry {
        name: "Sunset Photo".to_string(),
        object: photo_id.clone(),
        object_type: ObjectType::Blob,
        shared: false,  // private by default
        app: Some("gallery".to_string()),
    }],
    version: 1,
}).await?;
```

The photo is stored on Alice's disk, signed with her key, and listed in her home index. It's not visible to anyone else.

When Alice wants to share it:

```rust
runtime.share_object("Sunset Photo", &photo_id, Some("gallery".to_string())).await?;
```

This flips the `shared` flag and announces the object to the DHT. Now other peers can fetch it.

To unshare:

```rust
runtime.set_home_entry_shared("Sunset Photo", false).await?;  // by entry name
```

The object is unannounced and no longer served to peers.

---

## Story 3: Peer Discovery on LAN

**As a user, I want to automatically discover other Canopee users on my local network.**

Alice and Bob are on the same Wi-Fi network. Both have Canopee-powered apps running.

Alice's app starts and begins mDNS discovery. Bob's app does the same. Within seconds:

```
Alice sees: Peer 12D3KooWBOB... (no username yet)
Bob sees: Peer 12D3KooWALICE... (no username yet)
```

No configuration. No manual connection. mDNS broadcasts a query, the other peer responds, and a libp2p connection is established automatically.

They can now exchange objects directly:

```rust
// Alice fetches an object from Bob (network is a public field; takes values)
let bundle = runtime.network
    .get_object(bob_peer_id, object_id)
    .await?;

// Verify and store
runtime.import(bundle).await?;
```

---

## Story 4: Claiming a Username

**As a user, I want a human-readable name instead of a long peer ID.**

Alice claims a username:

```bash
canopee username claim alice
```

Under the hood:
1. Creates a `UsernameRecord { username: "alice", version: 1 }` signed by Alice's key
2. Publishes it as the record `(alice_identity, "username")`
3. Creates a registry entry `username:alice` → Alice's identity

Bob can now find Alice by name:

```bash
canopee username lookup alice
# Returns: canopee://identity/12D3KooWALICE...
```

Or in code:

```rust
let alice_identity = runtime.resolve_owner_from_username("alice").await?;
// Some(IdentityId("canopee://identity/12D3KooWALICE..."))
```

Resolution is spoof-verified: the `username:alice` registry entry points to an owner, and the resolver then loads that owner's *signed* `(owner, "username")` record and checks the claimed name matches (with `AppPointerRecord::verify()` binding the public key to the owner identity). If anything is wrong, resolution returns `None`.

---

## Story 5: Encrypted Chat

**As a user, I want to have a private, end-to-end encrypted conversation with a friend.**

Alice and Bob want to chat. They exchange contact strings out of band (in person, over an existing secure channel):

Alice's contact string:
```
canopee://identity/12D3KooWALICE...#dh=a1b2c3d4...
```

Bob's contact string:
```
canopee://identity/12D3KooWBOB...#dh=e5f6g7h8...
```

### Setup (Alice's side)

```rust
// Parse Bob's contact string (returns the DH key bytes; the caller
// assembles the full Contact). Chat app helper: parse_contact(s) -> [u8; 32]
let bob_dh_key = parse_contact(bob_input)?;

// Build the conversation — Conversation::new does agree() + HKDF internally
let conversation = Conversation::new(alice_identity.clone(), bob_dh_key);

// Derive the deterministic topic (chat app: conversation_topic(ours, theirs))
let topic = conversation_topic(&alice_dh_key, &bob_dh_key);
// Both sides compute the same topic independently
```

### Sending a Message

```rust
// Encrypt the message (a Conversation method)
let wire = conversation.encrypt("Hello Bob!".as_bytes());

// Publish to the deterministic topic (publish takes (topic, Vec<u8>))
runtime.network.publish(&topic, wire).await?;
```

### Receiving Messages

```rust
// subscribe returns a broadcast::Receiver; consume with .recv().await
let mut rx = runtime.network.subscribe(&topic).await?;

while let Ok(msg) = rx.recv().await {
    // Skip my own messages (source is Option<String>)
    if msg.source.as_deref() == Some(my_peer_id.as_str()) { continue; }

    // Decrypt
    match conversation.decrypt(&msg.data) {
        Ok(plaintext) => {
            println!("Bob: {}", String::from_utf8_lossy(&plaintext));
        }
        Err(_) => {
            // Not for us — different conversation using the same network
        }
    }
}
```

### What's Protected

- **Confidentiality**: only Alice and Bob can read the messages (they have the shared key)
- **Integrity**: ChaCha20-Poly1305 provides authenticated encryption
- **No server**: messages flow peer-to-peer through Gossipsub
- **No metadata protection**: an observer who knows both DH keys can see the topic

---

## Story 6: Publishing a Web Application

**As a developer, I want to publish my web app so anyone can run it locally.**

Bob builds a todo app and wants to distribute it:

### Build and Publish

```bash
# Build the app (standard web tooling)
npm run build  # produces ./build/

# Publish to Canopee
canopee app-manifest ./build todo-app
```

The CLI:
1. Walks `./build/`, creates Blob objects for each file
2. Identifies `index.html` as the entrypoint
3. Publishes the `AppManifest` to the DHT
4. Returns the manifest's ObjectId

### Alice Installs and Runs

```bash
# Resolve by owner + name
canopee open --owner canopee://identity/12D3KooWBOB... --name todo-app
```

Or by manifest ID:

```bash
canopee open <manifest-id>
```

The CLI:
1. Resolves the manifest from the DHT or a specific peer
2. Fetches all assets (index.html, style.css, app.js)
3. Starts a local HTTP server on `127.0.0.1:PORT`
4. Opens the browser to the local URL

Alice sees Bob's todo app running locally. No server, no deployment pipeline, no domain name.

### Caching

When Carol runs the same app, she fetches from both Bob and Alice (who both have the assets). The more people use the app, the faster it loads for new users. This is the distributed cache — no CDN required.

---

## Story 7: Multi-Device Identity

**As a user, I want to use the same identity on my laptop and desktop.**

Alice uses Canopee on her laptop. She buys a new desktop and wants the same identity.

### Method 1: Key File Copy

```bash
# On laptop
cp ~/.canopee/identity/identity.key /secure-usb/

# On desktop
mkdir -p ~/.canopee/identity/
cp /secure-usb/identity.key ~/.canopee/identity/
```

Both machines now share the same identity. Objects on the laptop are not automatically on the desktop — Alice syncs them by publishing/sharing through the DHT, or by manually copying the `storage/` directory.

### Method 2: Encrypted Key

For extra security, Alice encrypts the key:

```bash
# On laptop (with passphrase)
CANOPEE_IDENTITY_PASS="correct-horse-battery-staple" canopee init

# Copy the encrypted key
cp ~/.canopee/identity/identity.key /secure-usb/

# On desktop
CANOPEE_IDENTITY_PASS="correct-horse-battery-staple" canopee init
# Fails if key exists — delete and re-init, or copy over
cp /secure-usb/identity.key ~/.canopee/identity/
```

Both machines decrypt the same key with the same passphrase and share the same identity.

---

## Story 8: Sharing Data Across Applications

**As a user, I want my data to be available to all my Canopee-powered applications.**

Alice has three Canopee apps installed:
1. A photo gallery
2. A chat app
3. A file manager

All three share the same user root (`~/.canopee`). When Alice creates a profile in the chat app:

```rust
runtime.save_profile(Profile {
    display_name: "Alice Smith".to_string(),
    dh_public_key: dh_key,
    avatar: Some(photo_object_id),
    version: 1,
})?;
```

The profile is stored in `~/.canopee/records/` and pointed to by `(alice_identity, "profile")`.

The photo gallery can read this profile:

```rust
let profile = runtime.load_profile(&alice_identity)?;
// Some(Profile { display_name: "Alice Smith", ... })
```

The file manager can list Alice's home index:

```rust
let home = runtime.load_home_index(&alice_identity)?;
// HomeIndex { entries: [...], ... }
```

No data migration. No API calls. No import/export. The shared user root makes it seamless.

---

## Story 9: Fetching Someone Else's Data

**As a user, I want to fetch data from another peer by their username.**

Alice wants to get Bob's profile. She knows his username:

```bash
# Look up Bob's identity
canopee username lookup bob
# Returns: canopee://identity/12D3KooWBOB...

# Fetch Bob's profile
canopee fetch 12D3KooWBOB... <profile-object-id>
```

Or in code:

```rust
// Resolve Bob's identity from username
let bob_identity = runtime.resolve_username("bob")?;

// Resolve Bob's profile record
let profile = runtime.resolve_pointer(&bob_identity, "profile")?;

// profile is a verified Profile object
println!("Bob's name: {}", profile.display_name);
```

The resolution goes through the DHT, and the record's signature is verified before trusting it. If someone tries to publish a fake profile for Bob, the signature check fails and the record is rejected.

---

## Story 10: Real-Time Communication

**As a user, I want to chat with peers in real time.**

Alice joins a chat room:

```bash
canopee chat general
```

She types a message and presses Enter. The CLI publishes it to the `general` topic via Gossipsub. Bob, also subscribed to `general`, sees the message appear:

```
> Hello everyone!
[12D3KooWBOB...] Hey Alice!
[12D3KooWCAROL...] Hi both!
```

Behind the scenes:
1. `subscribe("general")` opens a persistent subscription
2. Publishing sends the message to the Gossipsub mesh
3. The mesh propagates the message to all subscribers
4. The CLI prints incoming messages in real time

This works across the internet (through relay nodes and DHT discovery) and on LAN (through mDNS). No chat server. No message history. No accounts.

---

## Story 11: Sharing with Specific Peers

**As a user, I want to control exactly who can see my data.**

Alice has a private document she wants to share only with Bob:

```rust
// Store the document
let doc_id = runtime.put(&document_bytes)?;

// Share it — this announces to DHT
runtime.share_object("Tax Return 2024", &doc_id, "documents")?;
```

Wait — this shares it with *everyone*. That's not what Alice wants.

The current sharing model is binary: an object is either shared (visible to all peers who ask) or private (visible to nobody). For selective sharing, Alice would need to:

1. Keep the object private (not announced)
2. Send the ExportBundle directly to Bob via a private channel (like the encrypted chat)

```rust
// Export the object
let bundle = runtime.export(&doc_id)?;

// Encrypt the bundle for Bob
let encrypted = encrypt_for_peer(&bob_dh_key, &bundle)?;

// Send via private chat
chat.send_encrypted(&bob_peer_id, &encrypted).await?;
```

Bob receives the encrypted bundle, decrypts it, and imports it. The object was never announced to the DHT — it traveled through an encrypted channel.

This demonstrates Canopee's composable security: the storage layer provides signing and content addressing; the application layer provides encryption and access control.

---

## Story 12: Relay-Connected Peers

**As a user behind a NAT, I want to connect to peers through a relay.**

Alice is behind a corporate firewall that blocks incoming connections. She connects through a public relay:

```bash
canopee dial /ip4/relay.example.com/tcp/4001/p2p/12D3KooWRELAY...
canopee listen-via-relay /ip4/relay.example.com/tcp/4001/p2p/12D3KooWRELAY...
```

The swarm:
1. Connects to the relay node
2. Requests a circuit reservation (a virtual port on the relay)
3. Other peers can now reach Alice through the relay

DCUtR (Direct Connection Upgrade through Relay) then attempts to establish a direct connection:
1. The relay observes Alice's external address
2. Bob (also behind a NAT) gets his external address from his relay
3. Both exchange addresses through the relay
4. Both simultaneously attempt to connect to each other's external addresses
5. If the NAT mapping allows it, the direct connection succeeds
6. The relay connection is dropped

Alice and Bob now talk directly, with no relay in the path.

---

## Story 13: Updating a Published Application

**As a developer, I want to push an update to my published app.**

Bob published version 1 of his todo app. He's now built version 2:

```bash
# Publish the new version (same app name)
canopee app-manifest ./build-v2 todo-app
```

The CLI creates a new manifest with new asset ObjectIds and updates the `AppPointer` record:

```
(owner, "todo-app") → AppPointer { manifest: v2_manifest_id, ... }
```

Alice, who has version 1 running, checks for updates:

```rust
let latest = runtime.resolve_app_pointer(&bob_identity, "todo-app")?;
// Returns the v2 manifest
```

If Alice refreshes or restarts the app, she gets version 2 automatically. Old peers serving version 1 continue to work — the old manifest and assets are still in the store.

---

## Story 14: Inspecting the Object Store

**As a developer, I want to browse what's in my object store.**

```bash
canopee list
```

Output:
```
a1b2c3d4e5f67890... { size: 15234, content_type: "image/jpeg" }
f7e8d9c0b1a23456... { size: 2048, content_type: "application/octet-stream" }
...
```

To inspect a specific object:

```bash
canopee get a1b2c3d4e5f67890...
```

To export and examine:

```bash
canopee export a1b2c3d4e5f67890... > object.canopee
```

The `.canopee` file is an `ExportBundle` (bincode-serialized). It contains the complete object with signature and public key — everything needed to verify its authenticity.

---

## Story 15: Building a Tauri Desktop App

**As a developer, I want to build a desktop app with built-in peer-to-peer capabilities.**

Marcus builds a collaborative notes app using Tauri:

### Backend (src-tauri/src/lib.rs)

```rust
use canopee_runtime::Runtime;

struct AppState {
    runtime: Runtime,
}

#[tauri::command]
async fn create_note(
    state: State<'_, AppState>,
    title: String,
    content: String,
) -> Result<String, String> {
    let note = serde_json::json!({
        "title": title,
        "content": content,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });
    
    let note_bytes = serde_json::to_vec(&note).unwrap();
    let info = state.runtime.put(&note_bytes).map_err(|e| e.to_string())?;
    
    // Add to home index
    state.runtime.share_object(
        &title, &info.object_id, "notes"
    ).map_err(|e| e.to_string())?;
    
    Ok(info.object_id)
}

#[tauri::command]
async fn list_notes(
    state: State<'_, AppState>,
) -> Result<Vec<HomeEntry>, String> {
    let home = state.runtime.load_home_index(
        &state.runtime.identity().id()
    ).map_err(|e| e.to_string())?;
    
    Ok(home.entries.into_iter()
        .filter(|e| e.app == "notes")
        .collect())
}
```

### Setup

```rust
let config = Config::with_app_root(app_data_dir)
    .with_user_root(user_dir)
    .with_mdns(false);

let runtime = Runtime::open_with_config(config).await
    .map_err(|e| e.to_string())?;

app.manage(AppState { runtime });
```

### Result

Marcus's notes app has:
- Local storage (notes on disk)
- Peer-to-peer sharing (share notes with collaborators)
- Identity (his notes are signed and attributed)
- Multi-device support (same identity, same notes)
- No backend server (everything runs in the app process)

He ships a single binary. No database to install. No server to deploy. No API keys to manage. The app *is* the node.

---

## Story 16: Encrypted Contact Sharing

**As a user, I want to add a new contact for encrypted communication.**

Alice meets Carol at a conference. They want to chat privately.

### In Person Exchange

Alice opens her chat app and shows her contact string:

```
canopee://identity/12D3KooWALICE...#dh=f9e8d7c6...
```

Carol scans it (or types it). The chat app parses it:

```rust
let alice_contact = parse_contact_string(input)?;
// Contact { name: "Alice", peer_id: "12D3KooWALICE...", dh_public_key: [u8; 32] }
```

### Persisting the Contact

The chat app saves the contact to Carol's shared `ContactList`:

```rust
let mut contacts = runtime.load_contact_list(&my_identity)?;
contacts.contacts.push(Contact {
    name: "Alice".to_string(),
    peer_id: alice_contact.peer_id,
    dh_public_key: alice_contact.dh_public_key,
    note: "Met at RustConf 2024".to_string(),
});
contacts.version += 1;
runtime.save_contact_list(contacts)?;
```

This contact is now visible to all of Carol's Canopee-powered apps. The file manager, the gallery, any app that reads the `ContactList` record.

### Starting the Conversation

The chat app derives the shared secret and deterministic topic:

```rust
let shared_secret = carol_identity.agree(alice_contact.dh_public_key);
let key = derive_conversation_key(&shared_secret);
let topic = derive_topic(&carol_dh_key, &alice_dh_key);

// Subscribe to the topic
runtime.network().subscribe(&topic).await?;
```

Both Carol and Alice now have the same topic and the same key. They can send encrypted messages that only they can read.

---

## Story 17: Network Resilience

**As a user, I want my app to work even when the network is unavailable.**

Alice is on an airplane with no internet. She opens her Canopee-powered note-taking app:

1. **Local storage works**: all her notes are on disk in `~/.canopee/storage/`
2. **No network operations**: the swarm has no peers, but the app doesn't crash
3. **She writes notes**: `runtime.put(&note_bytes)` succeeds locally
4. **She browses existing notes**: `runtime.list()` reads from disk

When the plane lands and Wi-Fi returns:
1. mDNS discovers peers on the hotel network
2. The DHT becomes reachable through relay connections
3. Shared objects are announced to the DHT
4. Peer-sourced objects are available for fetch

The transition from offline to online is seamless. The application doesn't need to know — it just calls `runtime.put()` and `runtime.get()`, and the storage layer handles the rest.

---

## Story 18: Verifying Object Authenticity

**As a user, I want to verify that data I receive hasn't been tampered with.**

Bob receives an object from an unknown peer:

```rust
let bundle = runtime.network()
    .get_object(&peer_id, &object_id)
    .await?;

// The runtime automatically verifies:
// 1. id == sha256(bincode(payload))  — content integrity
// 2. ed25519_verify(public_key, signature, payload)  — authorship

runtime.import(&bundle)?;  // only succeeds if verification passes
```

If an attacker modifies the object's bytes:
- The content hash changes → the `ObjectId` no longer matches → rejected
- The signature no longer matches the modified payload → rejected

If an attacker forges an object with a different key:
- The signature doesn't verify under the claimed owner's public key → rejected
- The public key doesn't derive the claimed `PeerId` → rejected

Bob can be confident that the object he received is exactly what the original author created, with no modifications.

---

## Story 19: Publishing a Profile

**As a user, I want to publish my profile so others can find my information.**

Alice creates her profile:

```rust
runtime.save_profile(Profile {
    display_name: "Alice Smith".to_string(),
    dh_public_key: identity.dh_public_key().to_vec(),
    avatar: Some(avatar_object_id),
    version: 1,
})?;
```

This:
1. Creates a signed `Profile` object
2. Stores it in `~/.canopee/storage/`
3. Publishes the record `(alice_identity, "profile")` to the DHT
4. Caches the record locally for instant resolution

When Bob looks up Alice's profile:

```rust
let profile = runtime.resolve_pointer(&alice_identity, "profile")?;
// Profile { display_name: "Alice Smith", dh_public_key: [...], ... }
```

Resolution checks the local cache first (instant), then falls back to the DHT (bounded at 10 seconds). The record's signature is always verified.

---

## Story 20: Running Multiple Nodes for Testing

**As a developer, I want to test peer-to-peer interactions locally.**

```rust
#[tokio::test]
async fn test_object_exchange() {
    // Create two isolated environments
    let dir_a = tempdir().unwrap();
    let dir_b = tempdir().unwrap();
    
    let config_a = Config::with_root(dir_a.path());
    let config_b = Config::with_root(dir_b.path());
    
    // Start two runtimes
    let runtime_a = Runtime::open_with_config(config_a).await.unwrap();
    let runtime_b = Runtime::open_with_config(config_b).await.unwrap();
    
    // Connect them
    let addr_b = runtime_b.network().listen_addr();
    runtime_a.network().dial(addr_b).await.unwrap();
    
    // Wait for discovery
    wait_for_peer(&runtime_a, runtime_b.peer_id(), Duration::from_secs(10)).await;
    
    // Alice creates an object
    let obj_id = runtime_a.put(b"secret message").unwrap();
    runtime_a.share_object("Message", &obj_id, "test").unwrap();
    
    // Bob fetches it
    wait_for_providers(&runtime_b, &obj_id, Duration::from_secs(30)).await;
    let bundle = runtime_b.network()
        .get_object(runtime_a.peer_id(), &obj_id)
        .await
        .unwrap();
    
    // Verify
    runtime_b.import(&bundle).unwrap();
    let obj = runtime_b.get(&obj_id).unwrap();
    assert_eq!(obj.data, b"secret message");
}
```

Each runtime has its own identity, storage, and network stack. They discover each other via mDNS on localhost and exchange objects through the P2P protocol. This is how the entire Canopee test suite works — real swarms, real objects, real verification.

---

## Story 21: Publishing an Interest Profile

**As a user, I want to publish my interests so other peers can find me.**

Alice sets up her interest profile using the `spot` app (the `interest-discovery` project — see [Chapter 19](19-semantic-search.md)):

```bash
$ spot publish --name "Alice" --interest "music production" --interest "guitar"
published interest profile: 024e7d20...
```

Under the hood, three things happen:

1. **Store**: an `InterestProfile` object (`display_name`, `interests`, `version`, `updated_at`) is stored as a shared `Blob` object in Alice's store.
2. **Point**: the record `(alice_identity, "interest-discovery")` is published, so *any peer who knows just Alice's identity* can resolve her profile.
3. **Announce**: DHT *provider records* are published for each of her interests' locality-sensitive-hash bucket keys — this is the "I'm interested in music production" signal that lets others find her.

Alice is now discoverable to anyone using `spot`. Her profile is a signed, verified object, so no other peer can forge it or impersonate her interests.

When Alice wants to update her interests:

```bash
$ spot publish --name "Alice" --interest "music production" --interest "guitar" --interest "synths"
published interest profile: 9f31b2c8...
```

A new profile object is stored, the `(owner, "interest-discovery")` pointer is repointed at it, and the new buckets are announced. The `version` field increments.

If Alice's node restarts, she can re-announce without republishing (DHT provider records are per-session and are forgotten on restart):

```bash
$ spot refresh
announcements refreshed
```

She can inspect what she currently publishes:

```bash
$ spot me
canopee://identity/12D3KooWFYsj...
profile: Alice (v2)
  - music production
  - guitar
  - synths
```

---

## Story 22: Semantic Search Across Peers

**As a user, I want to find peers by free-text interest, ranked by relevance.**

Bob wants to find someone to make music with. Alice published her interest profile (Story 21) earlier. Bob searches:

```bash
$ spot search "music making"
   1.00  Alice  <12D3KooWFYsj...>
       1.00  music production
```

### How the search works

1. **Embed**: the query `"music making"` is tokenized ("music", "making") and each token is embedded as a deterministic 128-dim hashed-character-n-gram vector. The same text always produces the same vector, on every machine.

2. **Bucket**: each token vector is hashed through 8 random-hyperplane projections, producing 8 bucket keys (`sha256("interest:v<p>:b<id>")`). These are *virtual* keys — they never correspond to a stored object.

3. **Find**: the app calls `find_providers` for each bucket key and unions the results. Alice's node announced the buckets for "music" and "production" — both similar to "music" — so she's a candidate.

4. **Verify**: for each candidate, the app resolves `(peer, "interest-discovery")` → `fetch_object` → `import` (the node verifies the SHA-256 content hash and Ed25519 signature) → checks the object really belongs to that peer. A peer can advertise itself for topics it doesn't publish, but its *profile* must be genuine. Invalid candidates are skipped.

5. **Score**: the query tokens are cosine-scored against the peer's **published** interests. "music" vs "music production" scores high; "making" vs "guitar" scores lower. Results are ranked and capped.

### Precision controls

Bob can tighten the search:

```bash
$ spot search "music making" --min-sim 0.7
```

Only peers whose best matching interest clears 0.7 cosine similarity are returned. Lowering `--min-sim` widens recall.

### The negative case works too

A genuinely unrelated query does not match:

```bash
$ spot search "celestial navigation" --min-sim 0.6
no matches at similarity >= 0.6 for: "celestial navigation"
```

`guitar` and `music` share character-n-gram structure (morphology); `celestial navigation` shares none — so no buckets align and Alice doesn't appear.

### What makes it decentralized

- No shared ML model: vectors come from deterministic hashed n-grams, computed identically by every peer
- No index server: the "index" *is* the DHT's provider-record plane
- No trust: every result is adopted through Canopee's standard verification pipeline
- Privacy-compatible: only published, shared profiles are ever indexed

This is the general pattern for similarity search on Canopee: describe yourself in short text, announce your LSH buckets as provider records, and let queries meet you there — then verify, then score.
