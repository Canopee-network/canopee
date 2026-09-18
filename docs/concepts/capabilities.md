# Capabilities

A **capability** is a signed grant: *"the issuer lets the subject exercise
these permissions on that resource."* Because it's a self-contained signed
document, any peer — issuer, subject, or a server node enforcing it — can
verify it cryptographically, with no authorization server in the loop.

This is the technical-vision authorization layer (§5.5), implemented in
`crates/canopee-storage/src/capability.rs` and surfaced through the runtime
(`crates/canopee-runtime/src/capabilities.rs`) and CLI
(`canopee capability …`).

## The grant

```rust
Capability {
    issuer:      IdentityId,     // who is granting
    subject:     IdentityId,     // who is allowed
    resource:    Resource,       // what is being granted access to
    permissions: Vec<Permission>,// read | write | publish
    issued_at:   u64,            // unix seconds (0 = anytime)
    expires_at:  Option<u64>,    // unix seconds; None = no expiry
    // plus a content-derived CapabilityId, the issuer's public key, and
    // a signature over the canonical serialization of all of the above
}
```

### Permissions

- `read` — fetch / resolve the resource.
- `write` — modify it (upsert a pointer, update the record).
- `publish` — publish to a channel (used with `Channel` resources).

### Resources

- `object:<id>` — a single content-addressed object,
- `shared:<owner>:<name>` — a named entry in an owner's home index,
- `channel:<topic>` — a pub/sub topic.

The canonical string form (`Resource::canonical()`) is what gets signed and
what appears in the CLI (`canopee capability grant --resource object:…`).

## Why it's self-verifying

`Capability::issue`:

1. hashes the (subject, resource, permissions, issued_at, expires_at) fields
   into a content-derived `CapabilityId`,
2. signs `id || fields` with the issuer's identity key,
3. 🛍️ embeds the issuer's public key in the document.

So verification is two checks anyone can run offline:

- recompute the signature against the embedded public key, and
- recompute `CapabilityId` from the fields and confirm it matches.

Modifying any field, or submitting a doctored id, fails both. No lookup,
no trusted third party, no network call.

## The capability index

Grants the issuer has made are tracked in a signed `CapabilityIndex`
published under the **`(owner, "capabilities")`** record — the same immutable
object + mutable pointer pattern as every other user record. From it:

- `capability list` — show your grants,
- `capability revoke <id>` — mark a grant revoked,
- the runtime consults it in `check_capability` / `check_access`.

## Enforcement points in the runtime

`crates/canopee-runtime/src/capabilities.rs`:

| Function | Role |
|---|---|
| `grant_capability(subject, resource, permissions, expires_at)` | issue + persist into the index |
| `list_capabilities()` | return the signed index |
| `revoke_capability(id)` | mark a grant revoked in a fresh index |
| `check_capability(subject, resource, permission)` | verify signature + validity for one grant |
| `check_remote_revocation(...)` | re-check a peer's `(owner,"capabilities")` record so a revoked grant stops being honored |
| `check_access(subject, resource, permission)` | gate serving a resource on a valid, non-revoked capability |

Serving a shared name to a peer is gated through `check_access`: even if the
pointer is resolvable, only a subject holding a valid `read` capability on
`shared:<owner>:<name>` is served the object.

## Typical flow

1. Alice issues to Bob: `capability grant --subject canopee://identity/…bob… --resource shared:alice:report --permission read`.
2. The grant (+ its `(owner,"capabilities")` record update) is published; Bob
   receives the signed capability.
3. Bob fetches the shared entry; Alice's node `check_access`-es Bob's
   capability before serving.
4. Alice wants to stop granting: `capability revoke <id>` → publishes a fresh
   index; Bob's capability no longer verifies against the current
   `(owner,"capabilities")` record.

See the [Capabilities guide](../guides/capabilities.md) for a live walk-through,
and the [reference entry](../reference/capabilities.md) for full command/flat
details.