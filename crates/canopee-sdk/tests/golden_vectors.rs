//! Emits the cross-language golden vectors: for a representative value of
//! every `NodeCommand`, `NodeResponse` and shared wire type, the exact bincode
//! 1.x legacy bytes plus a language-neutral `serde_json` description of the
//! value.
//!
//! The TypeScript/Go/Python SDKs regenerate/re-assert against the same fixture
//! (`tests/fixtures/golden.json` in each repo, produced by this test), which
//! pins their encoders/decoders to the exact byte contract the real node
//! speaks.
//!
//! Run it with:
//!   CANOPEE_GOLDEN_DIR=/some/dir cargo test -p canopee-sdk --test golden_vectors
//!
//! Without the env var the test does nothing (kept in the tree as the single
//! source of truth for the fixture).

use canopee_identity::Identity;
use canopee_protocol::{
    DeviceInfo, NodeCommand, NodeResponse, PairingData, PairingQrData, PairingRecord, PeerInfo,
    PubSubMessage, RelayReservationInfo, SyncResult,
};
use canopee_sdk::{
    AppPointerRecord, Capability, CapabilityId, CapabilityIndex, Contact, ContactList, ExportBundle,
    HomeEntry, HomeIndex, Object, ObjectId, ObjectInfo, ObjectType, Permission, Profile, Resource,
};
use canopee_storage::{CapabilityEntry, ExportCapability, ObjectMetadata, ObjectPayload};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn u64(v: u64) -> u64 {
    v
}

/// Encodes `value` with the exact `bincode::serialize` call the SDK/transport
/// uses, and records (name, hex, value-json).
fn vector<T: Serialize>(out: &mut Vec<Value>, name: &str, value: &T) {
    let bytes = bincode::serialize(value).expect("bincode serialize");
    let json = serde_json::to_value(value).expect("json serialize");
    out.push(serde_json::json!({
        "name": name,
        "hex": to_hex(&bytes),
        "value": json,
    }));
}

#[test]
fn dump_golden_vectors() {
    let Some(dir) = std::env::var_os("CANOPEE_GOLDEN_DIR") else {
        eprintln!("CANOPEE_GOLDEN_DIR not set; skipping golden dump");
        return;
    };
    let mut out: Vec<Value> = Vec::new();

    let oid = |s: &str| ObjectId::new(s);
    let dh_key = [7u8; 32];
    let now = 1_700_000_000u64;

    // ---- shared value types ----

    vector(
        &mut out,
        "ObjectId",
        &ObjectId::new("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
    );

    let payload = ObjectPayload {
        owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAliceAliceAliceAliceAliceAliceAlice"),
        metadata: ObjectMetadata {
            created_at: now,
            size: 5,
            content_type: None,
        },
        object_type: ObjectType::Blob,
        data: b"hello".to_vec(),
    };
    vector(&mut out, "ObjectPayload", &payload);

    let object = Object {
        id: oid("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        payload,
        public_key: vec![1u8, 2, 3, 4],
        signature: vec![9u8, 8, 7],
    };
    vector(&mut out, "Object", &object);

    vector(
        &mut out,
        "ExportBundle",
        &ExportBundle {
            version: 1,
            object: object.clone(),
        },
    );

    vector(
        &mut out,
        "ObjectMetadata",
        &ObjectMetadata {
            created_at: now,
            size: 5,
            content_type: None,
        },
    );

    vector(
        &mut out,
        "ObjectInfo",
        &ObjectInfo {
            id: oid("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWBob"),
            size: 5,
            verified: true,
            name: Some("greeting".into()),
            object_type: ObjectType::Blob,
        },
    );

    vector(
        &mut out,
        "Profile",
        &Profile {
            display_name: "alice".into(),
            dh_public_key: dh_key,
            avatar: Some(oid("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")),
            version: 3,
        },
    );

    vector(
        &mut out,
        "Contact",
        &Contact {
            name: "bob".into(),
            peer_id: "12D3KooWContactBob".into(),
            dh_public_key: [9u8; 32],
            note: Some("from the pub".into()),
        },
    );

    vector(
        &mut out,
        "ContactList",
        &ContactList {
            contacts: vec![
                Contact {
                    name: "bob".into(),
                    peer_id: "12D3KooWContactBob".into(),
                    dh_public_key: [9u8; 32],
                    note: None,
                },
                Contact {
                    name: "carol".into(),
                    peer_id: "12D3KooWContactCarol".into(),
                    dh_public_key: [10u8; 32],
                    note: Some("work".into()),
                },
            ],
            version: 2,
        },
    );

    vector(
        &mut out,
        "HomeEntry",
        &HomeEntry {
            name: "pic.png".into(),
            object: oid("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
            object_type: ObjectType::Blob,
            shared: true,
            app: Some("sdk-test".into()),
        },
    );

    vector(
        &mut out,
        "HomeIndex",
        &HomeIndex {
            profile: Some(oid("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee")),
            contacts: None,
            entries: vec![HomeEntry {
                name: "pic.png".into(),
                object: oid("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
                object_type: ObjectType::Blob,
                shared: true,
                app: Some("sdk-test".into()),
            }],
            version: 5,
        },
    );

    // Signed records: minted with a scratch identity so public_key/signature
    // are real. Only the byte layout matters to other SDKs, not the validity.
    let scratch = std::env::temp_dir().join(format!("canopee_golden_{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let id_path = scratch.join("identity.key");
    let _ = std::fs::remove_file(&id_path);
    let identity = futures_block_on(Identity::create(id_path.to_str().unwrap())).unwrap();

    let pointer = AppPointerRecord::sign(
        &identity,
        "alice-portfolio",
        oid("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
    )
    .unwrap();
    vector(&mut out, "AppPointerRecord", &pointer);

    // ---- enums dump directly as a tagged variant ----

    vector(&mut out, "ObjectType", &ObjectType::AppManifest);
    vector(&mut out, "Permission", &Permission::Write);
    vector(
        &mut out,
        "Resource",
        &Resource::SharedName {
            owner: canopee_sdk::IdentityId::new("canopee://identity/alice"),
            name: "photos".into(),
        },
    );

    let capability = Capability::issue(
        &identity,
        canopee_sdk::IdentityId::new("canopee://identity/bob"),
        Resource::Channel("team-news".into()),
        vec![Permission::Read, Permission::Write],
        Some(now + 3600),
    )
    .unwrap();
    vector(&mut out, "Capability", &capability);

    vector(
        &mut out,
        "CapabilityIndex",
        &CapabilityIndex {
            entries: vec![CapabilityEntry {
                capability: capability.clone(),
                revoked: false,
            }],
            version: 1,
        },
    );

    vector(
        &mut out,
        "CapabilityId",
        &CapabilityId("cafebabe".into()),
    );

    vector(
        &mut out,
        "CapabilityEntry",
        &CapabilityEntry {
            capability: capability.clone(),
            revoked: true,
        },
    );

    vector(
        &mut out,
        "ExportCapability",
        &ExportCapability {
            version: 1,
            capability: capability.clone(),
        },
    );

    vector(
        &mut out,
        "PairingRecord",
        &PairingRecord {
            name: "profile".into(),
            object: object.clone(),
            pointer: pointer.clone(),
        },
    );

    vector(
        &mut out,
        "PairingData",
        &PairingData {
            version: 1,
            identity_key: vec![1u8, 2, 3],
            records: vec![PairingRecord {
                name: "profile".into(),
                object: object.clone(),
                pointer: pointer.clone(),
            }],
        },
    );

    vector(
        &mut out,
        "PeerInfo",
        &PeerInfo {
            peer_id: "12D3KooWPeerInfoKey".into(),
            identity: Some(canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice")),
            username: Some("alice".into()),
            display_name: Some("Alice".into()),
            addresses: vec![
                "/ip4/127.0.0.1/tcp/4001/p2p/12D3KooWPeerInfoKey".into(),
                "/ip6/::1/tcp/4002".into(),
            ],
        },
    );

    vector(
        &mut out,
        "PubSubMessage",
        &PubSubMessage {
            topic: "app-topic".into(),
            source: Some("12D3KooWPubSubSource".into()),
            data: b"ping".to_vec(),
        },
    );

    vector(
        &mut out,
        "RelayReservationInfo",
        &RelayReservationInfo {
            relay_peer_id: "12D3KooWRelayIdOne".into(),
            renewal: true,
            listen_addrs: vec!["/ip4/5.6.7.8/tcp/4001/p2p/12D3KooWRelayIdOne".into()],
        },
    );

    vector(
        &mut out,
        "DeviceInfo",
        &DeviceInfo {
            device_id: "12D3KooWDeviceAlpha".into(),
            device_name: "laptop".into(),
        },
    );

    vector(
        &mut out,
        "PairingQrData",
        &PairingQrData {
            version: 1,
            device_id: "12D3KooWNewDeviceQr".into(),
            device_name: "phone".into(),
            lan_addr: "/ip4/192.168.1.5/tcp/34567/p2p/12D3KooWNewDeviceQr".into(),
            code: "A1B2C3D4E5F6".into(),
            session_id: "session-xyz".into(),
        },
    );

    vector(
        &mut out,
        "SyncResult",
        &SyncResult {
            profile_updated: true,
            contacts_updated: false,
            devices_updated: true,
        },
    );

    // Same shape as `AppManifest`, but with a `BTreeMap` for its `assets` so
    // the bytes are deterministic (bincode emits HashMap entries in whichever
    // order the map iterates; on the wire the map is order-insensitive, and
    // the other SDKs document sorting keys for stable output).
    #[derive(Serialize)]
    struct ManifestForGolden {
        name: String,
        owner: canopee_sdk::IdentityId,
        entrypoint: ObjectId,
        assets: BTreeMap<String, ObjectId>,
    }

    vector(
        &mut out,
        "AppManifest",
        &ManifestForGolden {
            name: "alice-portfolio".into(),
            owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice"),
            entrypoint: oid("1111111111111111111111111111111111111111111111111111111111111111"),
            assets: BTreeMap::from([
                ("index.html".into(), oid("2222222222222222222222222222222222222222222222222222222222222222")),
                ("main.js".into(), oid("3333333333333333333333333333333333333333333333333333333333333333")),
            ]),
        },
    );

    // ---- every NodeCommand variant ----

    let obj = oid("4444444444444444444444444444444444444444444444444444444444444444");
    let obj2 = oid("5555555555555555555555555555555555555555555555555555555555555555");

    macro_rules! cmd {
        ($name:literal, $value:expr) => {
            vector(&mut out, $name, &$value)
        };
    }

    cmd!("cmd_Put", NodeCommand::Put { data: b"hello".to_vec(), name: None });
    cmd!("cmd_PutObject", NodeCommand::PutObject { data: b"pic".to_vec(), object_type: ObjectType::Blob, name: Some("pic.png".into()) });
    cmd!("cmd_Concat", NodeCommand::Concat { ids: vec![obj.clone(), obj2.clone()], name: Some("joined".into()) });
    cmd!("cmd_Get", NodeCommand::Get { id: obj.clone() });
    cmd!("cmd_List", NodeCommand::List);
    cmd!("cmd_DeleteObject", NodeCommand::DeleteObject { id: obj.clone() });
    cmd!("cmd_SetName", NodeCommand::SetName { id: obj.clone(), name: "greeting".into() });
    cmd!("cmd_Export", NodeCommand::Export { id: obj.clone() });
    cmd!("cmd_Import", NodeCommand::Import { bundle: ExportBundle { version: 1, object: object.clone() } });
    cmd!("cmd_Status", NodeCommand::Status);
    cmd!("cmd_Identity", NodeCommand::Identity);
    cmd!("cmd_Shutdown", NodeCommand::Shutdown);
    cmd!("cmd_Dial", NodeCommand::Dial { addr: "/ip4/1.2.3.4/tcp/4001/p2p/12D3KooWPeer".into() });
    cmd!("cmd_ListenViaRelay", NodeCommand::ListenViaRelay { relay_addr: "/ip4/5.6.7.8/tcp/4001/p2p/12D3KooWRelay".into() });
    cmd!("cmd_Publish", NodeCommand::Publish { topic: "app-topic".into(), data: b"ping".to_vec() });
    cmd!("cmd_Subscribe", NodeCommand::Subscribe { topic: "app-topic".into() });
    cmd!("cmd_Peers", NodeCommand::Peers);
    cmd!("cmd_RelayReservations", NodeCommand::RelayReservations);
    cmd!("cmd_FindProviders", NodeCommand::FindProviders { id: obj.clone() });
    cmd!("cmd_FetchObject", NodeCommand::FetchObject { peer_id: "12D3KooWFetchFrom".into(), id: obj.clone() });
    cmd!("cmd_Announce", NodeCommand::Announce { id: obj.clone() });
    cmd!("cmd_PublishAppPointer", NodeCommand::PublishAppPointer { name: "app".into(), manifest: obj.clone() });
    cmd!("cmd_ResolveAppPointer", NodeCommand::ResolveAppPointer { owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice"), name: "app".into() });
    cmd!("cmd_PublishPointer", NodeCommand::PublishPointer { name: "app:demo".into(), target: obj.clone() });
    cmd!("cmd_ResolvePointer", NodeCommand::ResolvePointer { owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice"), name: "app:demo".into() });
    cmd!("cmd_SaveProfile", NodeCommand::SaveProfile { profile: Profile { display_name: "alice".into(), dh_public_key: dh_key, avatar: None, version: 0 } });
    cmd!("cmd_LoadProfile", NodeCommand::LoadProfile);
    cmd!("cmd_SaveContactList", NodeCommand::SaveContactList { list: ContactList { contacts: vec![], version: 0 } });
    cmd!("cmd_LoadContactList", NodeCommand::LoadContactList);
    cmd!("cmd_SaveHomeIndex", NodeCommand::SaveHomeIndex { index: HomeIndex { profile: None, contacts: None, entries: vec![], version: 0 } });
    cmd!("cmd_LoadHomeIndex", NodeCommand::LoadHomeIndex);
    cmd!("cmd_SetHomeEntryShared", NodeCommand::SetHomeEntryShared { name: "pic.png".into(), shared: true });
    cmd!("cmd_DhPublicKey", NodeCommand::DhPublicKey);
    cmd!("cmd_PutObjectPublic", NodeCommand::PutObjectPublic { data: b"manifest".to_vec(), object_type: ObjectType::AppManifest, name: Some("manifest".into()) });
    cmd!("cmd_ShareObject", NodeCommand::ShareObject { name: "pic.png".into(), object: obj.clone(), app: Some("photos".into()), recipients: vec![[1u8; 32], [2u8; 32]] });
    cmd!("cmd_ClaimUsername", NodeCommand::ClaimUsername { username: "alice".into() });
    cmd!("cmd_ResolveUsername", NodeCommand::ResolveUsername { username: "alice".into() });
    cmd!("cmd_ResolveProfile", NodeCommand::ResolveProfile { owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice") });
    cmd!("cmd_ExportIdentity", NodeCommand::ExportIdentity { passphrase: "hunter2".into() });
    cmd!("cmd_ImportIdentity", NodeCommand::ImportIdentity { bytes: vec![1u8, 2, 3], passphrase: "hunter2".into(), overwrite: true });
    cmd!("cmd_ShowUsername", NodeCommand::ShowUsername);
    cmd!("cmd_Device", NodeCommand::Device);
    cmd!("cmd_DeviceList", NodeCommand::DeviceList);
    cmd!("cmd_ResolveOwnerDevice", NodeCommand::ResolveOwnerDevice { owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice") });
    cmd!("cmd_AddDevice", NodeCommand::AddDevice { device_id: "12D3KooWDeviceBeta".into(), device_name: "tablet".into() });
    cmd!("cmd_RemoveDevice", NodeCommand::RemoveDevice { device_id: "12D3KooWDeviceBeta".into() });
    cmd!("cmd_InitiatePairing", NodeCommand::InitiatePairing);
    cmd!("cmd_CompletePairing", NodeCommand::CompletePairing { qr: PairingQrData { version: 1, device_id: "12D3KooWNew".into(), device_name: "phone".into(), lan_addr: "/ip4/192.168.1.5/tcp/34567/p2p/12D3KooWNew".into(), code: "A1B2C3D4E5F6".into(), session_id: "s-1".into() }, code: "A1B2C3D4E5F6".into() });
    cmd!("cmd_SyncFromPeer", NodeCommand::SyncFromPeer { peer_id: "12D3KooWSyncPeer".into() });
    cmd!("cmd_SyncDeviceList", NodeCommand::SyncDeviceList);
    cmd!("cmd_GrantCapability", NodeCommand::GrantCapability { subject: canopee_sdk::IdentityId::new("canopee://identity/bob"), resource: Resource::Object(obj.clone()), permissions: vec![Permission::Read], expires_at: Some(u64(now + 3600)) });
    cmd!("cmd_ListCapabilities", NodeCommand::ListCapabilities);
    cmd!("cmd_RevokeCapability", NodeCommand::RevokeCapability { id: CapabilityId("cafebabe".into()) });
    cmd!("cmd_CheckCapability", NodeCommand::CheckCapability { capability: capability.clone() });
    cmd!("cmd_CheckAccess", NodeCommand::CheckAccess { subject: canopee_sdk::IdentityId::new("canopee://identity/bob"), permission: Permission::Read, resource: Resource::Object(obj.clone()) });
    cmd!("cmd_StartServeSession", NodeCommand::StartServeSession { edge_addr: "/ip4/9.9.9.9/tcp/4001/p2p/12D3KooWEdge".into(), app_id: obj.clone() });
    cmd!("cmd_StopServeSession", NodeCommand::StopServeSession);

    // ---- every NodeResponse variant ----

    macro_rules! res {
        ($name:literal, $value:expr) => {
            vector(&mut out, $name, &$value)
        };
    }

    res!("res_ObjectCreated", NodeResponse::ObjectCreated { id: obj.clone() });
    res!("res_DhPublicKey", NodeResponse::DhPublicKey { key: dh_key });
    res!("res_Concatenated", NodeResponse::Concatenated { id: obj.clone() });
    res!("res_NameSet", NodeResponse::NameSet);
    res!("res_Object", NodeResponse::Object { object: object.clone() });
    res!("res_Objects", NodeResponse::Objects { objects: vec![ObjectInfo { id: obj.clone(), owner: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice"), size: 5, verified: true, name: None, object_type: ObjectType::Blob }] });
    res!("res_ObjectDeleted", NodeResponse::ObjectDeleted);
    res!("res_Exported", NodeResponse::Exported { bundle: ExportBundle { version: 1, object: object.clone() } });
    res!("res_Imported", NodeResponse::Imported);
    res!("res_Status", NodeResponse::Status { identity: "canopee://identity/12D3KooWAlice".into(), objects: 3, peers: 1 });
    res!("res_Error", NodeResponse::Error { message: "something broke".into() });
    res!("res_Identity", NodeResponse::Identity { identity_id: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice") });
    res!("res_ShutdownAccepted", NodeResponse::ShutdownAccepted);
    res!("res_Dialed", NodeResponse::Dialed);
    res!("res_ListeningViaRelay", NodeResponse::ListeningViaRelay);
    res!("res_Published", NodeResponse::Published);
    res!("res_Subscribed", NodeResponse::Subscribed);
    res!("res_PubSub", NodeResponse::PubSub(PubSubMessage { topic: "app-topic".into(), source: None, data: b"ping".to_vec() }));
    res!("res_Peers", NodeResponse::Peers { peers: vec![PeerInfo { peer_id: "12D3KooWPeer".into(), identity: None, username: None, display_name: None, addresses: vec![] }] });
    res!("res_RelayReservations", NodeResponse::RelayReservations { reservations: vec![RelayReservationInfo { relay_peer_id: "12D3KooWRelay".into(), renewal: false, listen_addrs: vec![] }] });
    res!("res_Providers", NodeResponse::Providers { peer_ids: vec!["12D3KooWProviderA".into(), "12D3KooWProviderB".into()] });
    res!("res_Announced", NodeResponse::Announced);
    res!("res_AppPointerPublished", NodeResponse::AppPointerPublished);
    res!("res_AppPointer", NodeResponse::AppPointer { record: AppPointerRecord::sign(&identity, "app", obj.clone()).ok() });
    res!("res_PointerPublished", NodeResponse::PointerPublished);
    res!("res_Pointer", NodeResponse::Pointer { record: None });
    res!("res_ProfileSaved", NodeResponse::ProfileSaved { id: obj.clone() });
    res!("res_Profile", NodeResponse::Profile { profile: Some(Profile { display_name: "alice".into(), dh_public_key: dh_key, avatar: None, version: 1 }) });
    res!("res_ContactListSaved", NodeResponse::ContactListSaved { id: obj.clone() });
    res!("res_ContactList", NodeResponse::ContactList { list: Some(ContactList { contacts: vec![], version: 1 }) });
    res!("res_HomeIndexSaved", NodeResponse::HomeIndexSaved { id: obj.clone() });
    res!("res_HomeIndex", NodeResponse::HomeIndex { index: Some(HomeIndex { profile: None, contacts: None, entries: vec![], version: 1 }) });
    res!("res_UsernameClaimed", NodeResponse::UsernameClaimed);
    res!("res_Username", NodeResponse::Username { username: Some("alice".into()) });
    res!("res_UsernameOwner", NodeResponse::UsernameOwner { owner: Some(canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice")) });
    res!("res_IdentityExported", NodeResponse::IdentityExported { bytes: vec![9u8, 9, 9] });
    res!("res_IdentityImported", NodeResponse::IdentityImported { identity_id: canopee_sdk::IdentityId::new("canopee://identity/12D3KooWAlice") });
    res!("res_Device", NodeResponse::Device { peer_id: "12D3KooWDevice".into(), device_name: "laptop".into() });
    res!("res_DeviceList", NodeResponse::DeviceList { devices: vec![DeviceInfo { device_id: "12D3KooWDevice".into(), device_name: "laptop".into() }] });
    res!("res_OwnerDevice", NodeResponse::OwnerDevice { peer_id: Some("12D3KooWDevice".into()) });
    res!("res_DeviceAdded", NodeResponse::DeviceAdded);
    res!("res_DeviceRemoved", NodeResponse::DeviceRemoved);
    res!("res_PairingQr", NodeResponse::PairingQr { qr: PairingQrData { version: 1, device_id: "12D3KooWNew".into(), device_name: "phone".into(), lan_addr: "/ip4/192.168.1.5/tcp/34567/p2p/12D3KooWNew".into(), code: "A1B2C3D4E5F6".into(), session_id: "s-1".into() } });
    res!("res_PairingComplete", NodeResponse::PairingComplete { message: "paired".into() });
    res!("res_SyncComplete", NodeResponse::SyncComplete { result: SyncResult { profile_updated: true, contacts_updated: false, devices_updated: true } });
    res!("res_CapabilityGranted", NodeResponse::CapabilityGranted { capability: capability.clone() });
    res!("res_Capabilities", NodeResponse::Capabilities { index: Some(CapabilityIndex { entries: vec![CapabilityEntry { capability: capability.clone(), revoked: false }], version: 1 }) });
    res!("res_CapabilityRevoked", NodeResponse::CapabilityRevoked);
    res!("res_CapabilityCheck", NodeResponse::CapabilityCheck { valid: true, reason: "ok".into() });
    res!("res_AccessAllowed", NodeResponse::AccessAllowed { allowed: true });
    res!("res_ServeSessionStarted", NodeResponse::ServeSessionStarted { app_id: obj.to_string(), root_url: "https://aabb.cc/serve/root".into() });
    res!("res_ServeSessionStopped", NodeResponse::ServeSessionStopped);
    res!("res_AppPointer.None", NodeResponse::AppPointer { record: None });

    let payload = serde_json::json!({
        "description": "Canopee bincode 1.x legacy vectors (little-endian fixint, u32 enum tags, u64 len prefixes). The `hex` field is the exact bincode::serialize output the node/SDK exchange over the Unix socket; the `value` field is the language-neutral description the other SDKs rebuild from.",
        "vectors": out,
    });

    let path = std::path::Path::new(&dir).join("golden.json");
    std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap()).unwrap();
    eprintln!("golden vectors written to {}", path.display());
}

/// No-frills block_on so this test doesn't need its own tokio runtime.
fn futures_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(future)
}