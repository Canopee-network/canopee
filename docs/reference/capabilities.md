# Capabilities reference

Capabilities are signed grants: *issuer → subject, permission(s) over a
resource*. Full concept in [Concepts → Capabilities](../concepts/capabilities.md).
This page is the flat, machine-checkable reference.

## Command surface

```
canopee cap grant <SUBJECT> <RESOURCE> [--read] [--write] [--publish] [--expires <UNIX_SECONDS>]
canopee cap list
canopee cap revoke <ID>
canopee cap check [BUNDLE] [--subject S] [--permission P] [--resource R]
```

## Resources (canonical string forms)

| Form | Meaning |
|---|---|
| `object:<id>` | a single content-addressed object |
| `channel:<topic>` | a pub/sub topic |
| `shared:<owner>:<name>` | a named entry in an owner's home index |

Parser: `Resource::parse` (crates/canopee-storage/src/capability.rs). These
canonical forms are what get signed and what `--resource` accepts.

## Permissions

| Flag | `Permission` | `as_str` | Meaning |
|---|---|---|---|
| `--read` | `Read` | `read` | fetch/resolve the resource |
| `--write` | `Write` | `write` | modify/upsert |
| `--publish` | `Publish` | `publish` | publish to the channel |

At least one flag must be given to `cap grant`.

## Grant fields

| Field | Type | Notes |
|---|---|---|
| `id` | `CapabilityId` | content-derived digest of (subject, resource, permissions, issued_at, expires_at); unchanged by the embedded public key/signature |
| `issuer` | `IdentityId` | `canopee://identity/<peer-id>` whoever signs (and who is *the owner* for `shared:` resources) |
| `subject` | `IdentityId` | who may exercise the grant |
| `resource` | `Resource` | the target (forms above) |
| `permissions` | `Vec<Permission>` | 1–3 of read/write/publish |
| `issued_at` | `u64` (unix seconds) | `0` = "valid from the epoch" |
| `expires_at` | `Option<u64>` | `None` = no expiry; default for `cap grant` |

The `public_key` (issuer's) and `signature` fields are embedded but **not**
part of the signing bytes; the signature is over the canonical serialization of
`id || fields`. Verification is fully offline: recompute the signature with the
embedded key, recompute the id from the fields.

## Revocation

`cap list` shows grants with revocation state; `cap revoke <id>` publishes a
fresh `(owner, "capabilities")` index marking that `CapabilityId` revoked.
Enforcement re-checks `check_remote_revocation` so a revoked grant stops being
honored even if the signed capability document still exists.

## Runtime/SDK hooks

| Function (runtime) | SDK | Role |
|---|---|---|
| `grant_capability` | `grant_capability(subject, resource, permissions, expires_at)` | issue + persist |
| `list_capabilities` | `list_capabilities()` | signed index |
| `revoke_capability` | `revoke_capability(id)` | mark revoked |
| `check_capability` | `check_capability(...)` | verify a grant |
| `check_access` | `check_access(...)` | gate serving a resource |

## Typical flows

1. **Grant read on a shared name**
   ```
   canopee cap grant canopee://identity/...bob... shared:alice:report --read
   ```
2. **Verify a subject's current standing**
   ```
   canopee cap check --subject canopee://identity/...bob... \
       --permission read --resource shared:alice:report
   ```
3. **Verify a bundle passed out of band**
   ```
   canopee cap check <base64-bundle>
   ```
4. **Revoke**
   ```
   canopee cap revoke <ID>
   ```