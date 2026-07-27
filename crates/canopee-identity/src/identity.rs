use anyhow::Result;
use libp2p::identity::{Keypair, PeerId};
use serde::{Deserialize, Serialize};
use tokio::fs;

#[allow(unused)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityId(String);

impl ToString for IdentityId {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

#[allow(unused)]
#[derive(Debug)]
pub struct Identity {
    signing_key: Keypair,
    pub identity_id: IdentityId,
}

impl Identity {
    pub fn id(&self) -> &IdentityId {
        &self.identity_id
    }

    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        Ok(self.signing_key.sign(data)?)
    }

    pub fn verify(&self, data: &[u8], signature: &[u8]) -> bool {
        self.signing_key.public().verify(data, signature)
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.signing_key.public().encode_protobuf()
    }

    pub async fn create(path: &str) -> Result<Self> {
        let signing_key = Keypair::generate_ed25519();
        let peer_id = PeerId::from(signing_key.public());

        let identity_id = format!("canopee://identity/{}", peer_id);

        let bytes = signing_key.to_protobuf_encoding()?;
        fs::write(path, bytes).await?;

        Ok(Self {
            signing_key,
            identity_id: IdentityId(identity_id),
        })
    }

    pub async fn load(path: &str) -> Result<Self> {
        let bytes = fs::read(path).await?;
        let signing_key = Keypair::from_protobuf_encoding(&bytes).expect("invalid keypair file");

        let peer_id = PeerId::from(signing_key.public());

        let identity_id = format!("canopee://identity/{}", peer_id);

        Ok(Self {
            signing_key,
            identity_id: IdentityId(identity_id),
        })
    }
}

#[tokio::test]
async fn identity_can_sign_and_verify() {
    let identity = Identity::create("./test.key").await.unwrap();

    let message = b"hello canopee";

    let signature = identity.sign(message).unwrap();

    assert!(identity.verify(message, &signature));
}
