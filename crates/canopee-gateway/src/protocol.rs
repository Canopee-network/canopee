//! JSON wire types for the gateway: a browser sends [`GatewayCommand`]s and
//! receives [`GatewayEvent`]s, both tagged by an `"op"` string.
//!
//! The types here are deliberately browser-shaped rather than a mirror of
//! the bincode [`NodeCommand`]s: ids are plain strings, byte arrays are
//! base64, and only the commands considered safe for a web page are exposed
//! (there is no `shutdown`, no low-level transport, nothing the node's own
//! CLI needs for administration).

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use canopee_identity::IdentityId;
use canopee_storage::{
    ContactList, HomeIndex, Object, ObjectId, ObjectMetadata, ObjectPayload, ObjectType, Profile,
};
use serde::{Deserialize, Serialize};

/// A command from the browser, tagged by `"op"`.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum GatewayCommand {
    // ---- identity ----
    Identity,

    // ---- storage ----
    /// Stores UTF-8 text as a signed object owned by the node's identity.
    Put {
        text: String,
    },
    /// Reads a locally stored object.
    Get {
        id: String,
    },
    /// Lists everything in local storage.
    List,
    /// Exports an object as a bundle (returns the object itself).
    Export {
        id: String,
    },
    /// Imports an object bundle into local storage.
    Import {
        bundle: ExportBundleView,
    },

    // ---- network ----
    /// Peers currently connected to the node.
    Peers,
    /// Dials a peer directly (`/ip4/.../tcp/.../p2p/<peer id>`).
    Dial {
        addr: String,
    },
    /// Announces on the DHT that this node holds `id`.
    Announce {
        id: String,
    },
    /// Finds peers that have announced `id`.
    FindProviders {
        id: String,
    },
    /// Fetches an object directly from a peer.
    FetchObject {
        peer_id: String,
        id: String,
    },

    // ---- pub/sub ----
    /// Publishes UTF-8 text to a gossipsub topic.
    Publish {
        topic: String,
        text: String,
    },
    /// Streams pub/sub messages on `topic` as [`GatewayEvent::PubSub`] events.
    Subscribe {
        topic: String,
    },

    // ---- app pointers ----
    /// Signs + publishes `(owner, name) -> manifest` from the node's identity.
    PublishAppPointer {
        name: String,
        manifest: String,
    },
    /// Resolves the latest manifest id published by `owner` under `name`.
    ResolveAppPointer {
        owner: String,
        name: String,
    },

    // ---- user records ----
    PublishPointer {
        name: String,
        target: String,
    },
    ResolvePointer {
        owner: String,
        name: String,
    },
    LoadProfile,
    SaveProfile {
        profile: ProfileView,
    },
    LoadContactList,
    SaveContactList {
        list: ContactListView,
    },
    LoadHomeIndex,
    SaveHomeIndex {
        index: HomeIndexView,
    },
    /// Flips one home entry's `shared` flag.
    SetHomeEntryShared {
        name: String,
        shared: bool,
    },
    /// Upserts a `shared: true` home entry and announces `object`.
    ShareObject {
        name: String,
        object: String,
        app: Option<String>,
    },

    // ---- usernames ----
    ClaimUsername {
        username: String,
    },
    ShowUsername,
    ResolveUsername {
        username: String,
    },

    // ---- identity transfer (loopback-only, same security posture as the
    //      Unix-socket node commands these mirror) ----
    /// Exports the identity key as an encrypted, transferable envelope.
    ExportIdentity {
        passphrase: String,
    },
    /// Imports an identity key from an exported envelope. The identity is
    /// written to disk; the node must be restarted for it to take effect.
    ImportIdentity {
        #[serde(rename = "dataB64")]
        data_b64: String,
        passphrase: String,
        overwrite: bool,
    },
}

/// An event sent back to the browser, tagged by `"op"`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum GatewayEvent {
    /// Successful request/response command. `result` carries the payload,
    /// whose shape is documented per command in `client.js` and the tutorial.
    Ok { result: serde_json::Value },
    /// The command failed (either it was rejected as unsafe or the node
    /// returned an error).
    Error { message: String },
    /// A pub/sub message pushed to a client with an active subscription.
    PubSub {
        topic: String,
        source: Option<String>,
        text: String,
        #[serde(rename = "dataB64")]
        data_b64: String,
    },
}

impl GatewayEvent {
    /// Builds an "ok" event with a JSON payload.
    pub fn ok(result: serde_json::Value) -> Self {
        GatewayEvent::Ok { result }
    }

    pub fn error(message: impl Into<String>) -> Self {
        GatewayEvent::Error {
            message: message.into(),
        }
    }
}

// ---- shared views ----

/// A browser-friendly view of a stored [`Object`]: ids as strings, byte
/// arrays as base64. Carries enough to reconstruct the object for import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectView {
    pub id: String,
    pub owner: String,
    pub object_type: String,
    pub size: u64,
    pub created_at: u64,
    pub content_type: Option<String>,
    /// The payload bytes as UTF-8 text (base64 is authoritative for binary).
    pub text: String,
    #[serde(rename = "dataB64")]
    pub data_b64: String,
    #[serde(rename = "publicKeyB64")]
    pub public_key_b64: String,
    #[serde(rename = "signatureB64")]
    pub signature_b64: String,
}

impl ObjectView {
    pub fn from_parts(
        id: ObjectId,
        payload: &ObjectPayload,
        public_key: &[u8],
        signature: &[u8],
    ) -> Self {
        ObjectView {
            id: id.0,
            owner: payload.owner.to_string(),
            object_type: format!("{:?}", payload.object_type),
            size: payload.metadata.size,
            created_at: payload.metadata.created_at,
            content_type: payload.metadata.content_type.clone(),
            text: String::from_utf8_lossy(&payload.data).to_string(),
            data_b64: B64.encode(&payload.data),
            public_key_b64: B64.encode(public_key),
            signature_b64: B64.encode(signature),
        }
    }

    pub fn reconstruct(&self) -> anyhow::Result<Object> {
        let object_type = parse_object_type(&self.object_type)?;
        let data = B64.decode(&self.data_b64)?;
        let public_key = B64.decode(&self.public_key_b64)?;
        let signature = B64.decode(&self.signature_b64)?;
        Ok(Object {
            id: ObjectId::new(&self.id),
            payload: ObjectPayload {
                owner: IdentityId::new(&self.owner),
                metadata: ObjectMetadata {
                    created_at: self.created_at,
                    size: self.size,
                    content_type: self.content_type.clone(),
                },
                object_type,
                data,
            },
            public_key,
            signature,
        })
    }
}

/// A browser-friendly bundle view, matching [`ExportBundle`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportBundleView {
    pub version: u32,
    pub object: ObjectView,
}

impl ExportBundleView {
    pub fn reconstruct(&self) -> anyhow::Result<Object> {
        self.object.reconstruct()
    }
}

impl From<&Object> for ObjectView {
    fn from(object: &Object) -> Self {
        ObjectView::from_parts(
            object.id.clone(),
            &object.payload,
            &object.public_key,
            &object.signature,
        )
    }
}

/// A slim view of a resolved [`canopee_storage::AppPointerRecord`]: the
/// verifiable public facts, nothing of the signing material.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerView {
    pub owner: String,
    pub name: String,
    pub manifest: String,
    pub published_at: u64,
}

pub fn pointer_to_view(
    owner: impl AsRef<str>,
    name: impl AsRef<str>,
    manifest: &ObjectId,
    published_at: u64,
) -> PointerView {
    PointerView {
        owner: owner.as_ref().to_string(),
        name: name.as_ref().to_string(),
        manifest: manifest.0.clone(),
        published_at,
    }
}

// ---- user records ----

/// A browser-friendly view of a [`Profile`]; the DH key is base64.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub display_name: String,
    #[serde(rename = "dhPublicKeyB64")]
    pub dh_public_key_b64: String,
    pub avatar: Option<String>,
    pub version: u64,
}

impl From<&Profile> for ProfileView {
    fn from(profile: &Profile) -> Self {
        ProfileView {
            display_name: profile.display_name.clone(),
            dh_public_key_b64: B64.encode(profile.dh_public_key),
            avatar: profile.avatar.as_ref().map(|a| a.0.clone()),
            version: profile.version,
        }
    }
}

impl TryFrom<ProfileView> for Profile {
    type Error = anyhow::Error;

    fn try_from(view: ProfileView) -> anyhow::Result<Self> {
        let bytes = B64.decode(&view.dh_public_key_b64)?;
        let dh_public_key: [u8; 32] = bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("dhPublicKeyB64 must decode to exactly 32 bytes"))?;
        Ok(Profile {
            display_name: view.display_name,
            dh_public_key,
            avatar: view.avatar.map(|a| ObjectId::new(&a)),
            version: view.version,
        })
    }
}

/// A browser-friendly view of one [`canopee_storage::Contact`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContactView {
    pub name: String,
    pub peer_id: String,
    #[serde(rename = "dhPublicKeyB64")]
    pub dh_public_key_b64: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContactListView {
    pub contacts: Vec<ContactView>,
    pub version: u64,
}

impl From<&ContactList> for ContactListView {
    fn from(list: &ContactList) -> Self {
        ContactListView {
            contacts: list
                .contacts
                .iter()
                .map(|c| ContactView {
                    name: c.name.clone(),
                    peer_id: c.peer_id.clone(),
                    dh_public_key_b64: B64.encode(c.dh_public_key),
                    note: c.note.clone(),
                })
                .collect(),
            version: list.version,
        }
    }
}

impl TryFrom<ContactListView> for ContactList {
    type Error = anyhow::Error;

    fn try_from(view: ContactListView) -> anyhow::Result<Self> {
        let mut contacts = Vec::with_capacity(view.contacts.len());
        for c in view.contacts {
            let bytes = B64.decode(&c.dh_public_key_b64)?;
            let dh_public_key: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("dhPublicKeyB64 must decode to exactly 32 bytes"))?;
            contacts.push(canopee_storage::Contact {
                name: c.name,
                peer_id: c.peer_id,
                dh_public_key,
                note: c.note,
            });
        }
        Ok(ContactList {
            contacts,
            version: view.version,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HomeEntryView {
    pub name: String,
    pub object: String,
    pub object_type: String,
    pub shared: bool,
    pub app: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HomeIndexView {
    pub version: u64,
    pub profile: Option<String>,
    pub contacts: Option<String>,
    pub entries: Vec<HomeEntryView>,
}

impl From<&HomeIndex> for HomeIndexView {
    fn from(index: &HomeIndex) -> Self {
        HomeIndexView {
            version: index.version,
            profile: index.profile.as_ref().map(|p| p.0.clone()),
            contacts: index.contacts.as_ref().map(|c| c.0.clone()),
            entries: index
                .entries
                .iter()
                .map(|e| HomeEntryView {
                    name: e.name.clone(),
                    object: e.object.0.clone(),
                    object_type: format!("{:?}", e.object_type),
                    shared: e.shared,
                    app: e.app.clone(),
                })
                .collect(),
        }
    }
}

impl TryFrom<HomeIndexView> for HomeIndex {
    type Error = anyhow::Error;

    fn try_from(view: HomeIndexView) -> anyhow::Result<Self> {
        let mut entries = Vec::with_capacity(view.entries.len());
        for e in view.entries {
            entries.push(canopee_storage::HomeEntry {
                name: e.name,
                object: ObjectId::new(&e.object),
                object_type: parse_object_type(&e.object_type)?,
                shared: e.shared,
                app: e.app,
            });
        }
        Ok(HomeIndex {
            version: view.version,
            profile: view.profile.map(|p| ObjectId::new(&p)),
            contacts: view.contacts.map(|c| ObjectId::new(&c)),
            entries,
        })
    }
}

fn parse_object_type(s: &str) -> anyhow::Result<ObjectType> {
    Ok(match s {
        "Blob" => ObjectType::Blob,
        "AppManifest" => ObjectType::AppManifest,
        "AppPointer" => ObjectType::AppPointer,
        "Profile" => ObjectType::Profile,
        "ContactList" => ObjectType::ContactList,
        "HomeIndex" => ObjectType::HomeIndex,
        "Capability" => ObjectType::Capability,
        "CapabilityIndex" => ObjectType::CapabilityIndex,
        other => anyhow::bail!("unknown object type \"{other}\""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_views_round_trip_through_json() {
        let command: GatewayCommand =
            serde_json::from_str(r#"{"op":"put","text":"hello"}"#).unwrap();
        assert!(matches!(command, GatewayCommand::Put { text } if text == "hello"));

        let command: GatewayCommand = serde_json::from_str(
            r#"{"op":"resolvePointer","owner":"canopee://identity/a","name":"app:demo"}"#,
        )
        .unwrap();
        assert!(matches!(
            command,
            GatewayCommand::ResolvePointer { owner, name }
                if owner == "canopee://identity/a" && name == "app:demo"
        ));

        let event = GatewayEvent::ok(serde_json::json!({ "id": "abc" }));
        let wire = serde_json::to_string(&event).unwrap();
        assert!(wire.contains("\"op\":\"ok\""), "{wire}");
    }

    #[test]
    fn object_view_round_trips_through_reconstruct() {
        let payload = ObjectPayload {
            owner: IdentityId::new("canopee://identity/test"),
            metadata: ObjectMetadata {
                created_at: 123,
                size: 3,
                content_type: None,
            },
            object_type: ObjectType::Blob,
            data: b"abc".to_vec(),
        };
        let object = Object {
            id: ObjectId::from_payload(&payload),
            payload,
            public_key: vec![1, 2, 3],
            signature: vec![4, 5, 6],
        };
        let view = ObjectView::from(&object);
        let rebuilt = view.reconstruct().unwrap();
        assert_eq!(rebuilt.id, object.id);
        assert_eq!(rebuilt.payload.data, b"abc");
        assert_eq!(rebuilt.public_key, vec![1, 2, 3]);
        assert_eq!(rebuilt.signature, vec![4, 5, 6]);
        assert_eq!(rebuilt.payload.metadata.created_at, 123);
    }

    #[test]
    fn user_record_views_round_trip() {
        let profile = Profile {
            display_name: "Alice".into(),
            dh_public_key: [7u8; 32],
            avatar: Some(ObjectId::new("avatar-id")),
            version: 2,
        };
        let view = ProfileView::from(&profile);
        let back: Profile = Profile::try_from(view).unwrap();
        assert_eq!(back, profile);

        let home = HomeIndex {
            version: 1,
            profile: None,
            contacts: None,
            entries: vec![canopee_storage::HomeEntry {
                name: "pic".into(),
                object: ObjectId::new("obj"),
                object_type: ObjectType::Blob,
                shared: true,
                app: Some("chat".into()),
            }],
        };
        let view = HomeIndexView::from(&home);
        let back: HomeIndex = HomeIndex::try_from(view).unwrap();
        assert_eq!(back, home);
    }

    #[test]
    fn gateway_event_tag_is_camel_cased() {
        let event = GatewayEvent::PubSub {
            topic: "t".into(),
            source: None,
            text: "hi".into(),
            data_b64: "aGk=".into(),
        };
        let wire = serde_json::to_string(&event).unwrap();
        assert!(wire.contains("\"op\":\"pubSub\""), "{wire}");
        assert!(wire.contains("\"dataB64\""), "{wire}");
    }
}
