# `canopee://` URIs

The `canopee://` URI scheme lets you link directly into the network from the
OS and browser — click a link, open an app, resolve a peer — with the same
feel as `mailto:` or `ssh://`.

## Register and unregister the scheme

```bash
canopee uri-register      # OS-level: canopee:// links route to `canopee handle`
canopee uri-unregister    # undo
```

Registration writes the OS-level handler so clicking `canopee://…` invokes
`canopee handle <uri>`. This is macOS/Linux-specific.

## How `handle` resolves a URI

```bash
canopee handle "canopee://alice/alice-portfolio"
```

1. `alice` is looked up in the local alias map (`~/.canopee/aliases`); if
   it's a username, it's first reverse-resolved via the DHT (see
   [Usernames](usernames.md)).
2. The resolved `IdentityId` + the name (`alice-portfolio`) are passed to
   `canopee open --owner <id> --name alice-portfolio`.
3. `open` resolves the `(owner, "app:<name>")` pointer, fetches the
   manifest, and serves it in the default browser.

## Local aliases

Aliases are the local shorthand that makes the scheme human-friendly:

```bash
canopee alias set alice "canopee://identity/12D3KooW...alice-owner"
canopee alias list
canopee alias remove alice
```

Stored in `~/.canopee/aliases` (see [Filesystem layout](../reference/filesystem.md)).
Aliases are local — they mean something only on your machine; usernames are
global. The two compose: after resolving a username you can `canopee alias
set` it for URI use on your own machine.

## Pending URIs

If the node isn't running when a `canopee://` link is clicked, the scheme
handler writes the URI to `~/.canopee/uri-pending` so it isn't silently
dropped. `canopee open` picks it up on next start.