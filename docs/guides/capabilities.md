# Capabilities in practice

A quick hands-on tour of signing, issuing, verifying, and revoking
capabilities — granting a specific peer controlled access to a resource
(see [Capabilities concept](../concepts/capabilities.md) for the model and
[reference](../reference/capabilities.md) for every flag).

## Why you'd use this

Sharing makes an object *available*; capabilities decide *who can read it*.
When you want fine-grained access — "Bob may read `shared:alice:report`,
nothing else, until next month" — you issue a capability instead of relying
on plain sharing alone.

Identity references use the canonical form. On one machine you can run two
isolated nodes (`CANOPEE_APP_ROOT=/tmp/a`, `/tmp/b`) to simulate Alice and
Bob; each prints its identity with `canopee identity`.

## Step 1: issue a grant

Alice grants Bob `read` on her shared name `report`:

```bash
# machine A (Alice):
canopee cap grant canopee://identity/...bob... shared:alice:report --read
```

The grant is signed by Alice's identity, stored in Alice's
`(owner, "capabilities")` index, and the issued capability is returned so
Alice can hand it to Bob out of band.

## Step 2: list and verify

```bash
# machine A (Alice):
canopee cap list     # every grant with its revocation state

# check Bob's current standing on a specific permission/resource:
canopee cap check --subject canopee://identity/...bob... \
    --permission read --resource shared:alice:report

# verify a bundle passed out of band:
canopee cap check <base64-bundle>
```

`check` verifies signature + id + issuer, the time window, and (when the
issuer's index is reachable) revocation state — the same answer the node
runs before serving the resource (`check_access`).

## Step 3: expire and revoke

```bash
# with an expiry (unix seconds) — default is never:
canopee cap grant canopee://identity/...bob... shared:alice:report \
    --read --expires 1710000000

# revoke an earlier grant by its content-derived id:
canopee cap revoke <capability-id>
```

Revoking publishes a fresh `(owner, "capabilities")` index; the runtime
re-checks revocation before honoring a presented grant
(`check_remote_revocation`), so an old signed document stops working.

## Resources you can grant on

| `--resource` form | What it grants access to |
|---|---|
| `object:<id>` | a single content-addressed object |
| `channel:<topic>` | a pub/sub topic |
| `shared:<owner>:<name>` | a named entry in an owner's home index |

## In an app (SDK)

```rust
// issuer side:
client.grant_capability(subject, Resource::SharedName { owner, name }, vec![Permission::Read], None).await?;

// verify a presented bundle:
client.check_capability(capability).await?;

// gate a resource before serving:
client.check_access(subject, Permission::Read, resource).await?;
```

For a worked end-to-end access-control example integrated into an app, the
[Tauri tutorials](tauri-quickstart.md) show the surrounding embedded-runtime
patterns; the capability calls sit on top of exactly that foundation.