//! Replay / verification-engine stability tests (Phase 1 milestone):
//!
//! The storage layer is the point where untrusted bytes from the network are
//! turned into objects, so the audit focuses on two invariants:
//!   1. whatever `put_verified` writes, `get_verified` returns unchanged and
//!      signature-verified — and only that.
//!   2. a corrupted, re-signed, or wrongly-pathed object is *never* returned
//!      as valid; every tamper path ends in an error, never a wrong object.
//!
//! Every test uses a unique scratch directory under `std::env::temp_dir()`.

use canopee_identity::Identity;
use canopee_storage::{Object, ObjectId, ObjectType, Storage, Verify};
use std::path::PathBuf;
use std::sync::Arc;

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("canopee_replay_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn storage_in(dir: &PathBuf) -> Storage {
    let root = dir.join("store");
    std::fs::create_dir_all(&root).unwrap();
    Storage::new(root.to_str().unwrap())
}

async fn identity(tag: &str) -> Arc<Identity> {
    Arc::new(
        Identity::create(
            scratch_dir(&format!("key_{tag}"))
                .join("identity.key")
                .to_str()
                .unwrap(),
        )
        .await
        .unwrap(),
    )
}

#[tokio::test]
async fn put_get_roundtrip_returns_identical_verified_object() {
    let storage = storage_in(&scratch_dir("roundtrip"));
    let owner = identity("roundtrip").await;

    let object = Object::new(&owner, b"hello canopee".to_vec(), ObjectType::Blob);
    storage.put_verified(&object).await.unwrap();

    let back = storage.get_verified(&object.id).await.unwrap();
    assert!(back.verify(), "re-read object must still verify");
    assert!(back.verify_id(), "content address must hold");
    // Byte-for-byte identical payload: replay must never mangle the data.
    assert_eq!(back.payload.data, b"hello canopee");
    assert_eq!(
        bincode::serialize(&back).unwrap(),
        bincode::serialize(&object).unwrap()
    );
}

#[tokio::test]
async fn tampered_payload_on_disk_is_never_returned() {
    let storage = storage_in(&scratch_dir("tamper"));
    let owner = identity("tamper").await;

    let object = Object::new(&owner, b"honest bytes".to_vec(), ObjectType::Blob);
    storage.put_verified(&object).await.unwrap();

    // Flip one byte of payload.data directly on disk, behind the engine's
    // back. Re-reading must fail signature verification rather than yield the
    // tampered object.
    let path = storage.root() + "/" + &object.id.0;
    let mut bytes = tokio::fs::read(&path).await.unwrap();
    // Flip the *last* byte, which for a Blob object is inside payload.data
    // (tamper with the signed content, not just the file).
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    tokio::fs::write(&path, &bytes).await.unwrap();

    let err = storage.get_verified(&object.id).await.unwrap_err();
    assert!(
        err.to_string().contains("Invalid signature"),
        "tampered object must be rejected by signature check: {err}"
    );
}

#[tokio::test]
async fn key_mismatch_signature_fails_verification() {
    let storage = storage_in(&scratch_dir("keymismatch"));
    let alice = identity("key_alice").await;
    let bob = identity("key_bob").await;

    let alice_obj = Object::new(&alice, b"alice speaks".to_vec(), ObjectType::Blob);
    let bob_obj = Object::new(&bob, b"bob speaks".to_vec(), ObjectType::Blob);

    // Alice's object re-signed with Bob's key: the signature must not verify
    // against Alice's embedded public key, and put_verified must refuse it.
    let mut forged = alice_obj.clone();
    forged.signature = bob_obj.signature.clone();
    assert!(
        !forged.verify(),
        "a signature from another key must not verify"
    );
    assert!(
        storage.put_verified(&forged).await.is_err(),
        "put_verified must reject an object whose signature is from another key"
    );

    // A replay of a *foreign* object (bob's) planted at alice's path: even
    // though bob's bytes are validly signed, the id/path mismatch must be
    // rejected so the object is never handed back as alice's.
    storage.put_verified(&alice_obj).await.unwrap();
    let bob_bytes = bincode::serialize(&bob_obj).unwrap();
    tokio::fs::write(format!("{}/{}", storage.root(), alice_obj.id.0), &bob_bytes)
        .await
        .unwrap();
    let err = storage.get_verified(&alice_obj.id).await.unwrap_err();
    assert!(
        err.to_string().contains("mismatch"),
        "a foreign signed object under a wrong path must be rejected: {err}"
    );
    // Bob's own object under his own id still verifies trivially.
    storage.put_verified(&bob_obj).await.unwrap();
    assert!(storage.get_verified(&bob_obj.id).await.unwrap().verify());
}

#[tokio::test]
async fn content_addressing_and_wrong_id_path() {
    let storage = storage_in(&scratch_dir("contentaddr"));
    let owner = identity("contentaddr").await;

    let object = Object::new(&owner, b"addressed".to_vec(), ObjectType::Blob);
    assert!(
        object.verify_id(),
        "ObjectId::from_payload must match the object's own id"
    );
    assert_eq!(object.id, ObjectId::from_payload(&object.payload));
    assert_ne!(object.id, ObjectId::from_data(&object.payload.data));
    storage.put_verified(&object).await.unwrap();

    // Rewrite the same signed bytes under a *different* (renamed) path: the
    // engine must refuse to hand it back as that other id.
    let renamed = ObjectId::new("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
    let bytes = tokio::fs::read(format!("{}/{}", storage.root(), object.id.0))
        .await
        .unwrap();
    tokio::fs::write(format!("{}/{}", storage.root(), renamed.0), &bytes)
        .await
        .unwrap();

    let err = storage.get_verified(&renamed).await.unwrap_err();
    assert!(
        err.to_string().contains("mismatch"),
        "get of a renamed/wrong-id path must be rejected: {err}"
    );
    assert!(
        storage.get_verified(&object.id).await.is_ok(),
        "the object under its own correct path must still be returned"
    );
}

#[tokio::test]
async fn list_skips_dotfiles_and_stale_temp_files() {
    let storage = storage_in(&scratch_dir("dotfiles"));
    let owner = identity("dotfiles").await;

    let object = Object::new(&owner, b"kept".to_vec(), ObjectType::Blob);
    storage.put_verified(&object).await.unwrap();

    // A name sidecar (`.{id}.name`) and a half-written temp file from a
    // concurrent writer ({id} → `.{id}.canopee-tmp`) share the root layout;
    // neither may appear as an object. All are dotfiles, which `list` skips.
    storage.set_name(&object.id, "kept.txt").await.unwrap();
    let tmp = format!("{}/.{}.canopee-tmp", storage.root(), object.id.0);
    tokio::fs::write(&tmp, b"incomplete").await.unwrap();

    let list = storage.list().await.unwrap();
    assert_eq!(list.len(), 1, "dotfiles must be skipped: {list:?}");
    assert_eq!(list[0], object.id);
    assert_eq!(
        storage.read_name(&object.id).await.as_deref(),
        Some("kept.txt")
    );

    let listed = storage.list_objects().await.unwrap();
    assert_eq!(listed.len(), 1, "dotfiles must be skipped in list_objects");
}

#[tokio::test]
async fn repeated_puts_are_idempotent_and_consistent() {
    let storage = storage_in(&scratch_dir("reput"));
    let owner = identity("reput").await;

    let object = Object::new(&owner, b"stable id".to_vec(), ObjectType::Blob);
    storage.put_verified(&object).await.unwrap();
    // Re-putting identical bytes yields the same content address and must be
    // an atomic no-op on disk (temp-write + rename over the same target).
    storage.put_verified(&object).await.unwrap();

    let back = storage.get_verified(&object.id).await.unwrap();
    assert!(back.verify());
    assert_eq!(back.payload.data, b"stable id");
    // No stray temp files may survive the second, overwriting put.
    let dir = std::fs::read_dir(&storage.root()).unwrap();
    for entry in dir.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') && name.ends_with(".canopee-tmp") {
            panic!("stale temp file left behind: {name}");
        }
    }
}
