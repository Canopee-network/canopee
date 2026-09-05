# The `canopee://` URI scheme: friendly app addresses in a browser

A **`canopee://` URI** is a stable, human-shareable address for an app
published under an app manifest — the thing you'd send a friend instead of a
manifest id or an `--owner`/`--name` CLI command. Typing it into a *normal*
web browser works: the browser hands the URL to the OS, which launches
Canopee as the scheme handler, and Canopee resolves, fetches, serves, and
opens the app. The address ends up rendered at `http://127.0.0.1:<port>`,
exactly as `canopee open` does today.

This sounds like it should be harder than it is. The trick is that **the
browser never fetches `canopee://` content itself** — it just hands the URL
to your OS, which knows (because you registered it) that Canopee is the app
for that scheme. This is the same mechanism browsers use for `mailto:`,
`magnet:`, `spotify:`, and so on.

Read [`app-manifests.md`](app-manifests.md) first for the resolve → fetch →
serve details this doc builds on.

## The address format

```
canopee://<owner>/<name>
```

- `<owner>` is either the canonical
  `canopee://identity/<peer-id>` form (the identity string the node shows)
  or a **friendly short name** (e.g. `alice`) from your local aliases file.
- `<name>` is the app manifest name Alice published under
  (`portfolio`, `alice-react-app`, …).

Because a canonical owner is itself a `canopee://identity/...` URI, a full
canonical address reuses the scheme for its own authority segment:

```
canopee://identity/12D3KooW.../portfolio
```

Short names are local to your machine — `alice` means "the identity mapped
to that short name in `~/.canopee/aliases`". This is the same tradeoff the
rest of Canopee makes: the global namespace is `(owner, name)` where `owner`
is a public key, and friendly strings are a convenience for *you*.

> For *globally* unique friendly names — resolvable by any peer via the DHT,
> not just your local aliases file — see
> [`usernames.md`](usernames.md). A claimed username (`canopee username
> claim alice`) can be aliased locally for URI use, but the alias itself
> stays a per-machine convenience.

## Setup

### Register the scheme handler

One-time, per OS. This tells the OS that `canopee://` URLs belong to
`canopee handle`:

```bash
canopee uri-register
```

What it does per platform:

- **macOS:** creates `~/Applications/Canopee URI.app` — a minimal app bundle
  declaring the `canopee` URL scheme and executing `canopee handle <url>` —
  and registers it with LaunchServices (`lsregister`).
- **Linux:** writes
  `~/.local/share/applications/canopee-uri-handler.desktop` marking
  `x-scheme-handler/canopee` and runs `xdg-mime default`.
- **Windows:** writes `HKCU\Software\Classes\canopee` pointing
  `canopee handle "%1"` at the `canopee` executable (no admin needed).

Undo with:

```bash
canopee uri-unregister
```

### Add a short name (optional)

Map a friendly name to a canonical owner so people can share
`canopee://alice/portfolio` rather than
`canopee://identity/12D3KooW.../portfolio`:

```bash
canopee alias set alice "canopee://identity/12D3KooW..."
canopee alias list        # alice -> canopee://identity/12D3KooW...
canopee alias remove alice
```

The canonical form needs no setup — it resolves immediately.

## Using it

With a node running and the handler registered, typing
`canopee://alice/portfolio` into any browser address bar:

1. The browser has no `canopee://` fetcher, so it asks the OS to open the URL.
2. The OS launches the registered handler → `canopee handle "canopee://alice/portfolio"`.
3. `handle` resolves the owner (short name or canonical), looks up the latest
   app pointer published under that name on the DHT, fetches the entrypoint
   and assets (from a peer or local storage, using the exact
   [`fetch_app`](../crates/canopee-cli/src/app.rs) path `open` uses), and
   serves them on an ephemeral `127.0.0.1` port.
4. It opens the bound `http://127.0.0.1:<port>` URL in your default browser
   and keeps serving until killed.

The cache/announce behavior is identical to `canopee open` — fetched objects
are imported and re-announced best-effort, so repeated visits can come from
local storage or any provider, not necessarily Alice's node.

## Troubleshooting

- **"no alias X in ..."** — the short name isn't mapped. Either
  `canopee alias set X "<owner>"` or use the canonical form.
- **"Canopee node is not running"** — the handler saves the URI to
  `~/.canopee/uri-pending`, so the click isn't lost. Start a node
  (`canopee start`) and re-run the printed `canopee handle "<uri>"` command.
- **The browser says "no app to open" or the OS ignores `canopee://`** —
  the handler isn't registered (run `canopee uri-register`), or you're on an
  OS where the registration needs a re-login to take effect (macOS sometimes
  requires re-login after registering a new bundle).
- Availability is unchanged from `open`: the app must be findable — Alice's
  node online, a provider reachable, or the objects already cached locally.

## Relationship to HTTPS

This feature deliberately does **not** require TLS. The browser only ever
receives a plain `http://127.0.0.1:<port>` URL minted by the local `handle`
process — the `canopee://` scheme is a launch gesture, not a transport. See
[`roadmap-https-tls.md`](roadmap-https-tls.md) for why public WebPKI trust
for `canopee://` origins is structurally impossible in a stock browser, and
why the address-bar version doesn't need it.