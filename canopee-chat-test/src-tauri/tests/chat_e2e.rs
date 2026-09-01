//! End-to-end test of the app's full chat path, without a GUI or separate
//! node processes: two embedded `Runtime`s in one process, connected over
//! libp2p, exchanging an AEAD-encrypted message on a per-conversation topic.
//!
//! This is the Step 3 + Step 4 + Step 6 verification from the tutorial,
//! translated to an automated test: peers find each other, the topic
//! derivation matches on both sides, what crosses the network is ciphertext,
//! and the recipient's independent key derivation decrypts it correctly.

use canopee_chat_test_lib::chat::{format_contact, parse_contact, Contact, Conversation};
use canopee_network::Multiaddr;
use canopee_runtime::Runtime;
use std::time::Duration;
use tokio::time::{timeout, Instant};

async fn dial_target(runtime: &Runtime) -> Multiaddr {
    let peer_id = runtime.identity.keypair().public().to_peer_id();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(addrs) = runtime.network.listen_addresses().await {
            // Prefer the loopback address for a same-machine dial; fall back
            // to any reported address.
            let chosen = addrs
                .iter()
                .find(|a| a.to_string().contains("/ip4/127.0.0.1/"))
                .or_else(|| addrs.first())
                .cloned();
            if let Some(addr) = chosen {
                return addr.clone().with_p2p(peer_id).unwrap_or(addr);
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a listen address"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn wait_for_peers(runtime: &Runtime, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(peers) = runtime.network.peers().await {
            if peers.len() >= count {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} peer(s)"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn open_runtime(label: &str) -> Runtime {
    let dir = std::env::temp_dir().join(format!(
        "canopee_chat_e2e_{}_{}",
        label,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Runtime::open_with_root(dir).await.unwrap()
}

#[tokio::test]
async fn encrypted_chat_between_two_embedded_runtimes() {
    let alice_rt = open_runtime("alice").await;
    let bob_rt = open_runtime("bob").await;

    // ---- Step 3: out-of-band contact strings, DH keys included ----
    let alice_contact = format_contact(
        &alice_rt.identity.keypair().public().to_peer_id().to_base58(),
        &alice_rt.identity.dh_public_key(),
    );
    let bob_contact = format_contact(
        &bob_rt.identity.keypair().public().to_peer_id().to_base58(),
        &bob_rt.identity.dh_public_key(),
    );
    assert_ne!(parse_contact(&alice_contact).unwrap(), parse_contact(&bob_contact).unwrap());

    // ---- connect the two swarms ----
    let alice_target = dial_target(&alice_rt).await;
    let bob_target = dial_target(&bob_rt).await;
    alice_rt.network.dial(bob_target.clone()).await.unwrap();
    bob_rt.network.dial(alice_target.clone()).await.unwrap();
    wait_for_peers(&alice_rt, 1).await;
    wait_for_peers(&bob_rt, 1).await;

    // ---- Step 4 + 6: derive conversation, subscribe both sides ----
    let alice_conv = Conversation::new(
        &alice_rt.identity,
        Contact {
            name: "bob".to_string(),
            peer_id: bob_rt.identity.keypair().public().to_peer_id().to_base58(),
            dh_public_key: parse_contact(&bob_contact).unwrap(),
        },
    );
    let bob_conv = Conversation::new(
        &bob_rt.identity,
        Contact {
            name: "alice".to_string(),
            peer_id: alice_rt.identity.keypair().public().to_peer_id().to_base58(),
            dh_public_key: parse_contact(&alice_contact).unwrap(),
        },
    );
    assert_eq!(alice_conv.topic(), bob_conv.topic());

    let mut rx_bob = bob_rt.network.subscribe(alice_conv.topic()).await.unwrap();

    // ---- 6c: publish ciphertext only; deliver + decrypt on the other side ----
    let plaintext = "hello bob, this is the secret message";
    let wire = alice_conv.encrypt(plaintext.as_bytes()).unwrap();
    assert!(
        !wire.windows(plaintext.len()).any(|w| w == plaintext.as_bytes()),
        "plaintext must never appear on the wire"
    );

    // Wait for the gossipsub mesh, then publish (mirroring the retry pattern
    // in crates/canopee-network/tests/handshake.rs).
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut sent = false;
    while Instant::now() < deadline {
        if alice_rt
            .network
            .publish(alice_conv.topic(), wire.clone())
            .await
            .is_ok()
        {
            sent = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(sent, "publish should eventually succeed");

    // Bob receives and decrypts.
    let received = timeout(Duration::from_secs(5), rx_bob.recv())
        .await
        .expect("timed out waiting for delivery to bob")
        .unwrap();
    assert_eq!(
        received.data,
        wire,
        "what bob receives is exactly what alice published (ciphertext)"
    );
    let decrypted = bob_conv
        .decrypt(&received.data)
        .expect("bob must be able to decrypt alice's message");
    assert_eq!(decrypted, plaintext.as_bytes());

    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("canopee_chat_e2e_alice_{}", std::process::id())),
    );
    let _ = std::fs::remove_dir_all(
        std::env::temp_dir().join(format!("canopee_chat_e2e_bob_{}", std::process::id())),
    );
}