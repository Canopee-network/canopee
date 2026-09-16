use anyhow::Result;
use libp2p::identity::{Keypair, PeerId};
use tokio::fs;

/// A device's long-lived libp2p keypair, distinct from the shared
/// [`crate::Identity`].
///
/// The identity key is the *person* and is identical on every device that
/// shares an account; the device key is the *machine* and must never leave
/// the device it was minted on. The network `PeerId` is derived from the
/// device key, so several devices of one identity show up as distinct peers
/// (and can all be online at once) without colliding on the network.
pub struct DeviceKey {
    signing_key: Keypair,
    device_name: String,
}

impl std::fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("DeviceKey")
            .field("peer_id", &self.peer_id())
            .field("device_name", &self.device_name)
            .finish()
    }
}

impl DeviceKey {
    pub fn keypair(&self) -> Keypair {
        self.signing_key.clone()
    }

    pub fn peer_id(&self) -> PeerId {
        PeerId::from(self.signing_key.public())
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Generates a fresh device key, persisting it to `path` (protobuf
    /// encoding, like the identity key).
    pub async fn create(path: &str, device_name: &str) -> Result<Self> {
        let key = Self::generate(device_name);
        let bytes = key.signing_key.to_protobuf_encoding()?;
        fs::write(path, bytes).await?;
        Ok(key)
    }

    pub fn generate(device_name: &str) -> Self {
        Self {
            signing_key: Keypair::generate_ed25519(),
            device_name: device_name.to_string(),
        }
    }

    pub async fn load(path: &str, device_name: &str) -> Result<Self> {
        let bytes = fs::read(path).await?;
        let signing_key = Keypair::from_protobuf_encoding(&bytes)
            .map_err(|e| anyhow::anyhow!("invalid device key file: {e}"))?;
        Ok(Self {
            signing_key,
            device_name: device_name.to_string(),
        })
    }

    /// Loads the device key at `path`, minting and persisting a fresh one
    /// only if none exists yet.
    ///
    /// Race-safe like [`crate::Identity::create_if_absent`]: the key file is
    /// created with `create_new`, so at most one writer wins per boot and
    /// every loser adopts the winner's key. This is the "one device key per
    /// device" guarantee — two apps racing to provision the same user root
    /// must never mint two device identities.
    pub async fn load_or_create(path: &str, device_name: &str) -> Result<Self> {
        use std::io::ErrorKind;
        use tokio::io::AsyncWriteExt;

        let candidate = Self::generate(device_name);
        let bytes = candidate.signing_key.to_protobuf_encoding()?;
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .await
        {
            Ok(mut file) => {
                file.write_all(&bytes).await?;
                file.sync_all().await?;
                Ok(candidate)
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                // Another process/thread created the key while we were racing.
                // Give the winner a moment to finish writing, then adopt the
                // same key.
                for attempt in 0..25 {
                    match Self::load(path, device_name).await {
                        Ok(key) => return Ok(key),
                        Err(_) if attempt < 24 => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await
                        }
                        Err(e) => return Err(e),
                    }
                }
                unreachable!()
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[tokio::test]
async fn device_key_persists_and_reloads_with_same_peer_id() {
    let path = "./device_reload.key";
    let _ = std::fs::remove_file(path);

    let a = DeviceKey::load_or_create(path, "laptop").await.unwrap();
    let b = DeviceKey::load_or_create(path, "laptop").await.unwrap();

    assert_eq!(a.peer_id(), b.peer_id());
    assert_eq!(a.device_name(), "laptop");
    assert_eq!(
        a.peer_id().to_string(),
        libp2p::PeerId::from(a.keypair().public()).to_string()
    );
}

#[tokio::test]
async fn device_key_survives_concurrent_first_run() {
    let path = "./device_race.key";
    let _ = std::fs::remove_file(path);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let path = path.to_string();
        handles.push(tokio::spawn(async move {
            DeviceKey::load_or_create(&path, "laptop").await.unwrap().peer_id()
        }));
    }
    let mut ids: Vec<_> = Vec::new();
    for h in handles {
        ids.push(h.await.unwrap());
    }
    ids.dedup();
    assert_eq!(ids.len(), 1, "all racers must converge on one device key");
}

#[tokio::test]
async fn distinct_paths_mint_distinct_device_keys() {
    let path_a = "./device_a.key";
    let path_b = "./device_b.key";
    let _ = std::fs::remove_file(path_a);
    let _ = std::fs::remove_file(path_b);

    let a = DeviceKey::load_or_create(path_a, "a").await.unwrap();
    let b = DeviceKey::load_or_create(path_b, "b").await.unwrap();

    assert_ne!(a.peer_id(), b.peer_id(), "two devices must not share a PeerId");
}