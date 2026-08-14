use canopee_identity::{Identity, IdentityId};
use canopee_storage::{Object, ObjectId, ObjectType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactCard {
    pub identity_id: IdentityId,
    pub display_name: String,
    pub avatar: Option<ObjectId>,
}

/*
pub struct Object {
    pub id: ObjectId,
    pub payload: ObjectPayload,
    pub public_key: PublicKeyBytes,
    pub signature: Signature,
}
 */

impl ContactCard {
    pub fn new(identity_id: IdentityId, display_name: &str, avatar: Option<ObjectId>) -> Self {
        Self {
            identity_id,
            display_name: display_name.into(),
            avatar,
        }
    }

    pub fn update_display_name(&mut self, display_name: &str) {
        self.display_name = display_name.trim().into();
    }

    pub fn update_avatar(&mut self, avatar: Option<ObjectId>) {
        self.avatar = avatar;
    }

    pub fn bundle(&self, identity: &Identity) -> anyhow::Result<Object> {
        let bytes = bincode::serialize(&self)?;
        Ok(Object::new(identity, bytes, ObjectType::ContactCard))
    }
}
