use crate::Runtime;
use canopee_identity::IdentityId;
use canopee_storage::{
    Capability, CapabilityEntry, CapabilityId, CapabilityIndex, Permission, Resource,
    RECORD_CAPABILITIES,
};
use time::OffsetDateTime;

impl Runtime {
    /// Issues a signed `Capability` on behalf of this node's identity,
    /// granting `subject` `permissions` over `resource`, optionally expiring
    /// at `expires_at` (unix seconds).
    pub async fn grant_capability(
        &self,
        subject: IdentityId,
        resource: Resource,
        permissions: Vec<Permission>,
        expires_at: Option<u64>,
    ) -> anyhow::Result<Capability> {
        let capability =
            Capability::issue(&self.identity, subject, resource, permissions, expires_at)?;
        let object = capability.to_object(&self.identity)?;
        self.storage.put_verified(&object).await?;

        let mut index = self
            .load_capability_index()
            .await?
            .unwrap_or(CapabilityIndex {
                entries: vec![],
                version: 0,
            });
        index.entries.push(CapabilityEntry {
            capability: capability.clone(),
            revoked: false,
        });
        self.save_capability_index(&index).await?;
        Ok(capability)
    }

    /// Loads this node's capability index via the `(owner, "capabilities")`
    /// record. `None` until the first grant.
    pub async fn load_capability_index(&self) -> anyhow::Result<Option<CapabilityIndex>> {
        let object = self
            .resolve_owner_object(self.identity.id(), RECORD_CAPABILITIES)
            .await?;
        let Some(object) = object else {
            return Ok(None);
        };
        Ok(Some(object.decode()?))
    }

    /// Republishes the capability index under `(owner, "capabilities")`.
    async fn save_capability_index(&self, index: &CapabilityIndex) -> anyhow::Result<ObjectId> {
        let version = index.version + 1;
        let mut index = index.clone();
        index.version = version;
        let object = index.to_object(&self.identity)?;
        let id = object.id.clone();
        self.storage.put_verified(&object).await?;
        self.publish_pointer(RECORD_CAPABILITIES, id.clone())
            .await?;
        Ok(id)
    }

    /// Lists every capability this node's identity has issued, with its
    /// revocation state, via `(owner, "capabilities")`.
    pub async fn list_capabilities(&self) -> anyhow::Result<Option<CapabilityIndex>> {
        self.load_capability_index().await
    }

    /// Marks a previously issued capability as revoked and republishes the
    /// index.
    pub async fn revoke_capability(&self, id: &CapabilityId) -> anyhow::Result<()> {
        let mut index = self
            .load_capability_index()
            .await?
            .ok_or_else(|| anyhow::anyhow!("no capabilities issued yet"))?;
        let entry = index
            .entries
            .iter_mut()
            .find(|e| e.capability.id == *id)
            .ok_or_else(|| anyhow::anyhow!("no capability with id {id}"))?;
        entry.revoked = true;
        self.save_capability_index(&index).await?;
        Ok(())
    }

    /// Verifies a presented capability end to end: signature + content id +
    /// issuer derivation, time window, and revocation check (best-effort).
    pub async fn check_capability(
        &self,
        capability: &Capability,
    ) -> anyhow::Result<(bool, String)> {
        let now = OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
        if !capability.verify() {
            return Ok((false, "signature or content id invalid".to_string()));
        }
        if !capability.is_active_at(now) {
            return Ok((
                false,
                match capability.expires_at {
                    Some(exp) if now > exp => format!("expired at {exp}"),
                    _ => "not yet issued".to_string(),
                },
            ));
        }
        let revoked = if capability.issuer == *self.identity.id() {
            self.load_capability_index()
                .await?
                .and_then(|index| index.by_id(&capability.id).map(|entry| entry.revoked))
                .unwrap_or(false)
        } else {
            self.check_remote_revocation(&capability.issuer, &capability.id)
                .await
        };
        if revoked {
            return Ok((false, "revoked by the issuer".to_string()));
        }
        Ok((true, "grant verified".to_string()))
    }

    /// Best-effort, bounded lookup of whether `owner` has revoked `id`.
    async fn check_remote_revocation(&self, owner: &IdentityId, id: &CapabilityId) -> bool {
        const REVOKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
        let result = tokio::time::timeout(
            REVOKE_TIMEOUT,
            self.resolve_owner_object(owner, RECORD_CAPABILITIES),
        )
        .await;
        match result {
            Ok(Ok(Some(object))) => match object.decode::<CapabilityIndex>() {
                Ok(index) => index.by_id(id).map(|e| e.revoked).unwrap_or(false),
                Err(_) => false,
            },
            _ => false,
        }
    }

    /// The issuer-side authorization question: does `subject` currently hold
    /// an unrevoked, unexpired grant from this node's identity of `permission`
    /// on `resource`?
    pub async fn check_access(
        &self,
        subject: &IdentityId,
        permission: Permission,
        resource: &Resource,
    ) -> anyhow::Result<bool> {
        let Some(index) = self.load_capability_index().await? else {
            return Ok(false);
        };
        let now = OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
        Ok(index.grants(subject, resource, permission, now))
    }
}

use canopee_storage::ObjectId;
