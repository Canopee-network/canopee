//! The user-level data model: everything that belongs to a person rather
//! than to a single app, stored as signed, content-addressed `Object`s in
//! the shared user store.
//!
//! Records (mutable) point at the latest version of each immutable object:
//! `(owner, "profile")` → Profile, `(owner, "contacts")` → ContactList,
//! `(owner, "home")` → HomeIndex. Any app — or any new device, after the
//! identity key is imported — can resolve those records and fetch + verify
//! the objects, which is what makes the user's data *transferable across
//! their apps*.

use crate::{Object, ObjectId, ObjectType};
use canopee_identity::Identity;
use serde::{Deserialize, Serialize};

/// Record names under which user objects are published (`(owner, name)`
/// pointer keys). Treated as reserved: apps publish their own pointers under
/// `app:<name>` instead.
pub const RECORD_PROFILE: &str = "profile";
pub const RECORD_CONTACTS: &str = "contacts";
pub const RECORD_HOME: &str = "home";
/// The reserved record name under which a user publishes their globally
/// claimed username: `(owner, "username")` → a signed `UsernameRecord`
/// object. The username is unique network-wide (see the DHT registry key
/// `USERNAME_REGISTRY_PREFIX` below) and lets other peers discover and
/// address this identity by a friendly name instead of a raw peer id.
pub const RECORD_USERNAME: &str = "username";
/// DHT record key prefix for the global username registry: a mutable
/// Kademlia record `username:<lowercased-name>` → the verified canonical
/// owner (`canopee://identity/<peer-id>`), so anyone can reverse-resolve a
/// username to its owning identity without knowing the peer id up front.
pub const USERNAME_REGISTRY_PREFIX: &str = "username:";
/// The reserved record name under which a user publishes the list of devices
/// that currently carry their identity: `(owner, "devices")` → a signed
/// `DeviceList` object. Each of a user's devices re-registers itself on
/// startup, so the list is the authoritative "which machines am I on" view
/// for any of the user's apps on any device.
pub const RECORD_DEVICES: &str = "devices";
/// DHT record key prefix for the device registry: a mutable Kademlia record
/// `device:<network-peer-id>` → the verified canonical owner
/// (`canopee://identity/<peer-id>`), so anyone who sees a device on the
/// network (or a bare peer id out of band) can reverse-resolve which identity
/// it carries. This is the reverse of `(owner, "devices")`, which resolves
/// identity → devices.
pub const DEVICE_REGISTRY_PREFIX: &str = "device:";

/// A user's public self-description. `dh_public_key` lets any app derive
/// E2E conversation keys with the user without an extra lookup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub display_name: String,
    pub dh_public_key: [u8; 32],
    pub avatar: Option<ObjectId>,
    /// Incremented per publish; purely informational (the record timestamp
    /// is the authoritative ordering).
    pub version: u64,
}

impl Profile {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::Profile,
        ))
    }
}

/// A user's claimed, globally unique username. Published as a signed object
/// under the `(owner, "username")` record; the same `username` is announced
/// on the DHT (see `USERNAME_REGISTRY_PREFIX`) so others can resolve the
/// name back to this identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsernameRecord {
    pub username: String,
    /// Incremented per claim; purely informational (the record timestamp is
    /// the authoritative ordering).
    pub version: u64,
}

impl UsernameRecord {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::Profile,
        ))
    }
}

/// One contact, as shared out of band: the person's `canopee://identity`
/// reference plus the DH key their apps publish in their profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contact {
    pub name: String,
    pub peer_id: String,
    pub dh_public_key: [u8; 32],
    pub note: Option<String>,
}

/// A signed snapshot of a user's contacts. Editing is publishing a new
/// version and repointing `(owner, "contacts")`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContactList {
    pub contacts: Vec<Contact>,
    pub version: u64,
}

impl ContactList {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::ContactList,
        ))
    }

    /// Returns the (first) contact matching `peer_id`, if any.
    pub fn by_peer_id(&self, peer_id: &str) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.peer_id == peer_id)
    }
}

/// One named entry in a user's home index: a file, picture, or app the user
/// put in their store, referenced by object id.
///
/// `shared` controls *network* visibility: a `false` entry is only ever
/// served from the user's own store; setting `true` (via
/// `Runtime::set_home_entry_shared`, or by saving an index with the flag
/// set) is the explicit "share this on the network" action — the object is
/// announced on the DHT and served to any peer that asks. Nothing is
/// shared-by-default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HomeEntry {
    pub name: String,
    pub object: ObjectId,
    pub object_type: ObjectType,
    pub shared: bool,
    /// The app that added the entry ("chat", "game", ...), shown to the
    /// user as provenance.
    pub app: Option<String>,
}

/// The user's "home": where everything they've put in their store is listed,
/// together with the ids of their profile and contact-list objects. Any app
/// — or a fresh device after key import — resolves `(owner, "home")` to get
/// a table of contents for the whole user store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HomeIndex {
    pub profile: Option<ObjectId>,
    pub contacts: Option<ObjectId>,
    pub entries: Vec<HomeEntry>,
    pub version: u64,
}

impl HomeIndex {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::HomeIndex,
        ))
    }
}

/// One device currently carrying an identity: the device's *network* peer id
/// (from its `DeviceKey`, shared over the wire so other devices can dial and
/// fetch from it while it is online) plus a human-friendly name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceEntry {
    pub device_id: String,
    pub device_name: String,
    /// Opaque to the device itself; informational. See the `time` crate's
    /// serde support for the wire format.
    pub added_at: Option<time::OffsetDateTime>,
}

/// A signed snapshot of the devices a user's identity currently lives on.
/// Published under `(owner, "devices")`; editing is re-registering a device
/// and repointing the record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceList {
    pub devices: Vec<DeviceEntry>,
    /// Incremented per edit; purely informational (the record timestamp is
    /// the authoritative ordering).
    pub version: u64,
}

impl DeviceList {
    pub fn to_object(&self, identity: &Identity) -> anyhow::Result<Object> {
        Ok(Object::new(
            identity,
            bincode::serialize(self)?,
            ObjectType::Profile,
        ))
    }

    /// Returns the (first) entry matching `device_id`, if any.
    pub fn by_device_id(&self, device_id: &str) -> Option<&DeviceEntry> {
        self.devices.iter().find(|d| d.device_id == device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Verify;

    async fn identity() -> Identity {
        let dir = std::env::temp_dir().join(format!(
            "canopee_storage_user_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("identity.key");
        let path_str = path.to_str().unwrap();
        match canopee_identity::Identity::load(path_str).await {
            Ok(id) => id,
            Err(_) => {
                let _ = std::fs::remove_file(path_str);
                canopee_identity::Identity::create(path_str).await.unwrap()
            }
        }
    }

    #[tokio::test]
    async fn user_objects_roundtrip_through_disk_and_verify() {
        let clock = identity().await;
        let root = std::env::temp_dir().join(format!(
            "canopee_user_store_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = crate::Storage::new(root.to_str().unwrap());

        let profile = Profile {
            display_name: "Alice".into(),
            dh_public_key: clock.dh_public_key(),
            avatar: None,
            version: 1,
        };
        let p_object = profile.to_object(&clock).unwrap();
        assert_eq!(p_object.object_type(), ObjectType::Profile);
        assert!(p_object.verify());
        storage.put_verified(&p_object).await.unwrap();
        let back: Profile = storage
            .get_verified(&p_object.id)
            .await
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(back, profile);

        let contacts = ContactList {
            contacts: vec![Contact {
                name: "bob".into(),
                peer_id: "12D3KooBob".into(),
                dh_public_key: [9u8; 32],
                note: None,
            }],
            version: 1,
        };
        let c_object = contacts.to_object(&clock).unwrap();
        assert!(c_object.verify());
        storage.put_verified(&c_object).await.unwrap();
        let back: ContactList = storage
            .get_verified(&c_object.id)
            .await
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(back, contacts);

        let home = HomeIndex {
            profile: Some(p_object.id.clone()),
            contacts: Some(c_object.id.clone()),
            entries: vec![HomeEntry {
                name: "penguin.png".into(),
                object: ObjectId::new("abc"),
                object_type: ObjectType::Blob,
                shared: false,
                app: Some("chat".into()),
            }],
            version: 1,
        };
        let h_object = home.to_object(&clock).unwrap();
        assert!(h_object.verify());
        let back: HomeIndex = h_object.decode().unwrap();
        assert_eq!(back, home);
        assert!(!back.entries[0].shared, "entries default to not shared");
    }

    #[tokio::test]
    async fn device_list_roundtrips_through_disk_and_verify() {
        let clock = identity().await;
        let root = std::env::temp_dir().join(format!(
            "canopee_dev_list_store_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let storage = crate::Storage::new(root.to_str().unwrap());

        let devices = DeviceList {
            devices: vec![DeviceEntry {
                device_id: "12D3KooDeviceA".into(),
                device_name: "laptop".into(),
                added_at: None,
            }],
            version: 1,
        };
        assert!(
            devices.by_device_id("12D3KooDeviceA").is_some(),
            "lookup by device id should find the entry"
        );
        assert!(
            devices.by_device_id("nope").is_none(),
            "lookup must not match unrelated ids"
        );

        let d_object = devices.to_object(&clock).unwrap();
        assert!(d_object.verify());
        storage.put_verified(&d_object).await.unwrap();
        let back: DeviceList = storage
            .get_verified(&d_object.id)
            .await
            .unwrap()
            .decode()
            .unwrap();
        assert_eq!(back, devices);
    }
}