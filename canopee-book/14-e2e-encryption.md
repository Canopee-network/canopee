# Chapter 14: End-to-End Encryption

## The Pattern

Canopee does not provide built-in content encryption. But it provides the cryptographic primitives — X25519 Diffie-Hellman key agreement — that applications use to implement end-to-end encryption.

The chat application demonstrates this pattern. This chapter explains the mechanism.

## X25519 Key Agreement

From the same Ed25519 identity keypair, Canopee derives an X25519 public key:

```rust
let dh_public = identity.dh_public_key();  // [u8; 32]
```

Two peers who know each other's DH public keys can compute a shared secret:

```rust
let shared_secret = identity.agree(their_dh_public_key);  // [u8; 32]
```

The `agree` function performs the X25519 scalar multiplication. The result is a raw shared secret that **must** be processed through a key derivation function before use.

## Key Derivation

The raw Diffie-Hellman output is not suitable for direct use as an encryption key. It must be run through a KDF:

```rust
// Using HKDF-SHA256 (as in the chat app)
let hk = hkdf::Hkdf::<Sha256>::new(None, &shared_secret);  // no salt
let mut key = [0u8; 32];
hk.expand(b"canopee-chat/conversation-key/v1", &mut key).unwrap();  // domain as info
```

The domain separation string (`canopee-chat/conversation-key/v1`) is passed as the HKDF *info* parameter (not the salt), ensuring keys derived for different purposes are independent, even with the same shared secret.

## The Chat Encryption Protocol

### Key Exchange

1. Alice generates a contact string: `canopee://identity/<alice-peer-id>#dh=<alice-dh-pubkey-hex>`
2. Alice shares this string with Bob (out-of-band: in person, over an existing secure channel, etc.)
3. Bob parses the contact string to extract Alice's DH public key
4. Bob computes `shared_secret = bob_identity.agree(alice_dh_pubkey)`
5. Bob derives the conversation key using HKDF
6. Bob stores the contact in his `ContactList` record (which is shared across all his apps/devices)

Alice does the same with Bob's contact string. Both sides independently compute the same shared secret.

### Deterministic Topic Derivation

Both peers compute the same Gossipsub topic from their two DH **public keys** (not the shared secret — which is why an observer who knows both DH public keys can also derive the topic, as noted in the metadata caveat):

```rust
let topic = {
    let mut keys = vec![alice_dh_pubkey, bob_dh_pubkey];
    keys.sort();  // canonical ordering
    let mut hasher = Sha256::new();
    hasher.update(b"canopee-chat/topic/v1");  // TOPIC_DOMAIN separator
    hasher.update(keys.concat());
    hex::encode(hasher.finalize())  // bare hex digest, no prefix
};
```

Since both sides sort the keys the same way and hash the same bytes, they derive the same topic without any exchange. This is the "no coordination needed" property — once you have each other's DH keys, you can communicate.

### Message Encryption

Each message is encrypted with ChaCha20-Poly1305:

```rust
fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng); // random 12 bytes
    let ciphertext = cipher.encrypt(&nonce, plaintext).unwrap();
    [nonce.as_slice(), ciphertext.as_slice()].concat()
}

fn decrypt(key: &[u8; 32], wire: &[u8]) -> Result<Vec<u8>> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key));
    let nonce = Nonce::from_slice(&wire[..12]);
    let ciphertext = &wire[12..];
    Ok(cipher.decrypt(nonce, ciphertext)?)
}
```

The wire format is `nonce || ciphertext`. Each message uses a fresh random nonce, providing semantic security.

### Message Flow

```
Alice                                    Bob
  |                                       |
  |  subscribe(topic)                     |  subscribe(topic)
  |                                       |
  |--- publish(encrypt(msg)) ----------->|
  |                                       |  decrypt(msg)
  |                                       |
  |<---------- publish(encrypt(msg)) -----|
  |  decrypt(msg)                         |
  |                                       |
```

Messages are published to the deterministic Gossipsub topic. Only peers who have derived the same topic (i.e., peers who have each other's DH keys) receive and decrypt the messages.

## Domain Separation

The chat protocol uses domain-separated constants at every stage:

| Constant | Purpose |
|----------|---------|
| `canopee/dh/x25519/v1` | DH key derivation from Ed25519 |
| `canopee-chat/conversation-key/v1` | HKDF info for conversation key |
| `canopee-chat/topic/v1` | Gossipsub topic derivation |

Domain separation ensures that keys and identifiers used for different purposes cannot be confused or cross-used.

## Contact String Format

The contact string is a human-readable, shareable representation of a peer's identity:

```
canopee://identity/<peer-id>#dh=<x25519-pubkey-hex>
```

Example:
```
canopee://identity/12D3KooWABCD1234...#dh=a1b2c3d4e5f6...
```

The contact string contains everything needed to establish an encrypted conversation:
- The peer's identity (for routing and verification)
- The peer's DH public key (for key agreement)

Sharing a contact string is the out-of-band trust establishment step. How you share it (in person, over an existing secure channel, etc.) determines the security of the initial trust.

## What This Does NOT Provide

- **Forward secrecy**: if a conversation key is compromised, all past messages are revealed. There is no ratcheting mechanism.
- **Post-compromise security**: same as above — key rotation requires a new DH exchange.
- **Group encryption**: the protocol is pairwise. Group chats would require pairwise encryption for each participant.
- **Offline delivery**: messages are only delivered to online peers. There is no message queue or store-and-forward.
- **Metadata protection**: Gossipsub topic is deterministic from the DH keys. An observer who knows both DH keys can determine who is talking to whom.

These are all well-understood limitations that could be addressed in future versions (double ratcheting, group key management, offline storage, mixnet routing).
