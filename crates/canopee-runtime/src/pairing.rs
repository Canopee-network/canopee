use crate::Runtime;
use canopee_identity::Identity;
use canopee_network::{CanopeePairingRequest, CanopeePairingResponse, InboundPairing, Multiaddr, PeerId};
use canopee_protocol::{PairingData, PairingPayload, PairingQrData, PairingRecord};
use canopee_storage::{AppPointerRecord, RECORD_CONTACTS, RECORD_DEVICES, RECORD_PROFILE, Verify};
use time::OffsetDateTime;

impl Runtime {
    /// Picks the address the pairing QR advertises for the other device to
    /// dial: the first non-loopback listen address, falling back to loopback.
    async fn dialable_lan_addr(&self) -> anyhow::Result<String> {
        let addrs = self.network.listen_addresses().await?;
        let is_ip = |s: &str| s.starts_with("/ip4/") || s.starts_with("/ip6/");
        let is_loopback = |s: &str| s.starts_with("/ip4/127.") || s.starts_with("/ip6/::1");
        let pick = addrs
            .iter()
            .map(|a| a.to_string())
            .find(|s| is_ip(s) && !is_loopback(s))
            .or_else(|| addrs.iter().map(|a| a.to_string()).find(|s| is_ip(s)))
            .ok_or_else(|| anyhow::anyhow!("no listen address available to pair over"))?;
        let base = pick.split("/p2p/").next().unwrap_or(&pick);
        if !is_ip(base) {
            anyhow::bail!("listen address {pick} is not a dialable ip address");
        }
        Ok(format!("{}/p2p/{}", base, self.device_key.peer_id()))
    }

    /// Starts a device-pairing session on THIS device (the new device): mints
    /// a fresh 12-char pairing code + one-time session id and returns the
    /// `PairingQrData` to display or print out of band.
    pub async fn initiate_pairing(&self) -> anyhow::Result<PairingQrData> {
        let code = canopee_identity::pairing::generate_code();
        let session_id = canopee_identity::pairing::generate_session_id();
        let lan_addr = self.dialable_lan_addr().await?;

        *self.pairing.write().await = Some(crate::OwnPairing {
            session_id: session_id.clone(),
            code: code.clone(),
        });

        Ok(PairingQrData {
            version: 1,
            device_id: self.device_key.peer_id().to_string(),
            device_name: self.device_key.device_name().to_string(),
            lan_addr,
            code,
            session_id,
        })
    }

    /// The counterpart to [`Self::initiate_pairing`], run on the device that
    /// already carries the identity (the source).
    pub async fn complete_pairing(
        &self,
        qr: PairingQrData,
        code: &str,
    ) -> anyhow::Result<String> {
        if qr.version != 1 {
            anyhow::bail!("unsupported pairing protocol version {}", qr.version);
        }
        if !canopee_identity::pairing::codes_match(&qr.code, code) {
            anyhow::bail!(
                "pairing code does not match the code on the device — check it and retry"
            );
        }
        let new_device: PeerId = qr
            .device_id
            .parse()
            .map_err(|e| anyhow::anyhow!("device id in the QR is not a valid peer id: {e}"))?;
        if new_device == self.device_key.peer_id() {
            anyhow::bail!("this device is already the one carrying the identity — nothing to pair");
        }

        let session_key = self.pairing_session_key(&qr, new_device, code)?;
        let payload = self.encrypt_pairing_payload(&session_key).await?;

        let request = CanopeePairingRequest {
            from: self.device_key.peer_id().to_string(),
            device_id: qr.device_id.clone(),
            session_id: qr.session_id.clone(),
            payload,
        };
        self.send_pairing_request(new_device, &qr.lan_addr, request)
            .await
    }

    /// Derives the 256-bit pairing session key from the code + one-time
    /// session id, bound to both device ids.
    fn pairing_session_key(
        &self,
        qr: &PairingQrData,
        new_device: PeerId,
        code: &str,
    ) -> anyhow::Result<[u8; 32]> {
        let mut salt = Vec::from(b"canopee/pairing/v1");
        salt.extend_from_slice(qr.session_id.as_bytes());
        salt.extend_from_slice(self.device_key.peer_id().to_bytes().as_slice());
        salt.extend_from_slice(new_device.to_bytes().as_slice());
        Ok(canopee_identity::pairing::derive_session_key(code, &salt))
    }

    /// Builds + encrypts the pairing payload sent to the new device: this
    /// device's raw identity keypair bytes plus its signed user records.
    async fn encrypt_pairing_payload(
        &self,
        session_key: &[u8; 32],
    ) -> anyhow::Result<PairingPayload> {
        let identity_key = self.identity.export_bytes()?;

        let mut records = Vec::new();
        for name in [RECORD_DEVICES, RECORD_PROFILE, RECORD_CONTACTS] {
            let key = AppPointerRecord::key(self.identity.id(), name);
            let path = self
                .config
                .records_path()
                .join(format!("{}.record", hex::encode(&key)));
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(pointer) = bincode::deserialize::<AppPointerRecord>(&bytes) else {
                continue;
            };
            if pointer.owner != *self.identity.id() || !pointer.verify() {
                continue;
            }
            let Ok(object) = self.storage.get_verified(&pointer.manifest).await else {
                continue;
            };
            if !object.verify() || object.payload.owner != *self.identity.id() {
                continue;
            }
            records.push(PairingRecord {
                name: name.to_string(),
                object,
                pointer,
            });
        }

        let data = PairingData {
            version: 1,
            identity_key,
            records,
        };
        let encrypted = canopee_identity::pairing::encrypt_payload(
            &bincode::serialize(&data)?,
            session_key,
        )?;
        Ok(PairingPayload { encrypted })
    }

    /// Dials the new device's advertised LAN address and delivers the pairing
    /// request over `/canopee/pairing/1.0.0`.
    async fn send_pairing_request(
        &self,
        device: PeerId,
        lan_addr: &str,
        request: CanopeePairingRequest,
    ) -> anyhow::Result<String> {
        const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

        let addr: Multiaddr = lan_addr.parse()?;
        self.network.dial(addr).await?;

        for _ in 0..40 {
            if let Ok(peers) = self.network.peers().await {
                if peers.iter().any(|p| p.peer_id == device) {
                    return self.network.send_pairing(device, request).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        anyhow::bail!("the device at {lan_addr} did not come online within {DIAL_TIMEOUT:?}")
    }

    /// Handles one inbound `/canopee/pairing/1.0.0` request on the *new*
    /// device.
    async fn accept_pairing(&self, incoming: InboundPairing) -> CanopeePairingResponse {
        let session = self.pairing.write().await.take();
        let Some(session) = session.filter(|s| s.session_id == incoming.request.session_id) else {
            tracing::warn!(
                "ignored pairing request {}: no matching session on this device",
                incoming.request.session_id
            );
            return CanopeePairingResponse::Error("no pairing session for this request".into());
        };

        let from: PeerId = match incoming.request.from.parse() {
            Ok(peer) if peer == incoming.peer => peer,
            _ => {
                tracing::warn!("ignored pairing request: `from` does not match the sender");
                return CanopeePairingResponse::Error("sender identity mismatch".into());
            }
        };
        let target: PeerId = match incoming.request.device_id.parse() {
            Ok(peer) if peer == self.device_key.peer_id() => peer,
            _ => {
                tracing::warn!("ignored pairing request: not addressed to this device");
                return CanopeePairingResponse::Error("wrong destination device".into());
            }
        };

        let mut salt = Vec::from(b"canopee/pairing/v1");
        salt.extend_from_slice(incoming.request.session_id.as_bytes());
        salt.extend_from_slice(from.to_bytes().as_slice());
        salt.extend_from_slice(target.to_bytes().as_slice());
        let key = canopee_identity::pairing::derive_session_key(&session.code, &salt);
        let clear = match canopee_identity::pairing::decrypt_payload(
            &incoming.request.payload.encrypted,
            &key,
        ) {
            Ok(clear) => clear,
            Err(e) => {
                tracing::warn!("pairing request rejected: {e}");
                return CanopeePairingResponse::Error(e.to_string());
            }
        };
        let data: PairingData = match bincode::deserialize::<PairingData>(&clear) {
            Ok(data) if data.version == 1 => data,
            _ => {
                tracing::warn!("pairing request rejected: malformed payload");
                return CanopeePairingResponse::Error("malformed pairing payload".into());
            }
        };
        let imported = match Identity::import_bytes(&data.identity_key) {
            Ok(identity) => identity,
            Err(e) => return CanopeePairingResponse::Error(e.to_string()),
        };

        for record in &data.records {
            let valid = record.pointer.owner == *imported.id()
                && record.pointer.manifest == record.object.id
                && record.pointer.name == record.name
                && record.pointer.verify()
                && record.object.verify()
                && record.object.payload.owner == *imported.id();
            if !valid {
                let reason = "the transferred records do not verify against the transferred identity";
                tracing::warn!("pairing request rejected: {reason}");
                return CanopeePairingResponse::Error(reason.into());
            }
        }

        if let Err(e) = self.set_identity_key(&imported).await {
            tracing::warn!("pairing request rejected: {e}");
            return CanopeePairingResponse::Error(e.to_string());
        }
        for record in &data.records {
            if let Err(e) = self.storage.import(&record.object).await {
                tracing::warn!("pairing import of {:?} failed: {e}", record.object.id);
                return CanopeePairingResponse::Error(format!(
                    "record {} could not be stored: {e}",
                    record.name
                ));
            }
            self.cache.mark_cached(&record.object.id).await;
            let key = AppPointerRecord::key(imported.id(), &record.name);
            if let Ok(bytes) = bincode::serialize(&record.pointer) {
                if let Err(e) = self.cache_record(&key, &bytes).await {
                    tracing::warn!("pairing record cache write failed: {e}");
                }
            }
        }

        let message = format!(
            "accepted — restart {} ({} ) to take over the identity",
            self.device_key.device_name(),
            imported.id()
        );
        tracing::info!("{message}");
        CanopeePairingResponse::Accepted(message)
    }

    /// Writes `imported` to this device's identity key file, honoring the
    /// same at-rest policy as `open_with_config`.
    async fn set_identity_key(&self, imported: &Identity) -> anyhow::Result<()> {
        let identity_path = self.config.identity_path().join("identity.key");
        let exists = tokio::fs::try_exists(&identity_path).await.unwrap_or(false);
        if exists {
            let backup = self
                .config
                .identity_path()
                .join(format!(
                    "identity.key.bak-{}",
                    OffsetDateTime::now_utc().unix_timestamp()
                ));
            tokio::fs::copy(&identity_path, &backup).await?;
        }
        let passphrase: Option<String> = std::env::var_os("CANOPEE_IDENTITY_PASS")
            .and_then(|p| p.into_string().ok());
        let bytes = match passphrase.as_deref() {
            Some(pass) => imported.export_encrypted(pass)?,
            None => imported.export_bytes()?,
        };
        let tmp = self.config.identity_path().join("identity.key.tmp");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &identity_path).await?;
        Ok(())
    }

    /// Spawns the background task answering inbound `/canopee/pairing/1.0.0`
    /// requests for the lifetime of this runtime.
    pub(crate) fn spawn_pairing_handler(&self) {
        let runtime = self.clone();
        tokio::spawn(async move {
            let mut events = runtime.network.pairing_events();
            while let Ok(incoming) = events.recv().await {
                let reply = incoming.reply.clone();
                let response = runtime.accept_pairing(incoming).await;
                if reply.send(response).is_err() {
                    tracing::warn!("pairing reply channel closed before answering");
                }
            }
        });
    }
}