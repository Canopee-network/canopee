# Publishing a static app vs. building a Canopee app

Canopee has two completely different things that both get called "apps,"
and it's easy to conflate them because both end with something a user
interacts with. This doc draws the line between them: what each one is,
what it can and can't do, and where they meet.

If you haven't read [`app-manifests.md`](app-manifests.md) yet, do that
first — this doc assumes you know what publishing/opening a manifest does.

## Two different things, not two ways of doing the same thing

**A published app (`canopee app-manifest`)** is *static content* — a set
of pre-built files (HTML, CSS, JS, images) wrapped in an `AppManifest`
object and served back over plain HTTP by `canopee open`. Once published,
it's inert data sitting in someone's object storage. Nothing is running.
`open` fetches bytes and a small hand-rolled HTTP server
([`serve`](../crates/canopee-cli/src/app.rs)) hands them to a browser
byte-for-byte — exactly what any static host (Netlify, GitHub Pages, nginx)
does. It is exactly as dynamic as the JavaScript you bundled into it, and
not one bit more: a React SPA published this way can still fetch from
regular web APIs, animate, route client-side, whatever plain JS in a
browser can already do — it just can't reach back into Canopee itself,
because nothing bridges a browser tab to a running node (see
[Where they meet](#where-they-meet) below).

**A Canopee app (built on `canopee-sdk`)** is *a real program* — a process
that links against [`canopee-sdk`](../crates/canopee-sdk/README.md), runs
continuously or on demand, and talks to a locally running
[`canopee-node`](../crates/canopee-node/README.md) over its Unix socket
using [`canopee-protocol`](../crates/canopee-protocol/README.md). It's
alive: it can store and fetch objects, dial peers, announce/resolve things
on the DHT, publish and subscribe to gossipsub topics, and react to
whatever comes back — the same way `canopee-cli`'s own commands do,
because the CLI *is* one of these, just packaged as a shell tool instead
of a standalone binary.

The shortest way to say it: **a published app is content Canopee happens to
host. A `canopee-sdk` app is a peer — a first-class participant in the
network**, same standing as any `canopee-node`'s own CLI.

## Side by side

| | Published app (`app-manifest`) | `canopee-sdk` app |
|---|---|---|
| What it is | A signed `AppManifest` object + the files it points to | A process linking `canopee-sdk` |
| Where it runs | In a browser, fetched via `canopee open` | Wherever you run the binary — a server, a CLI, a background service |
| Lifecycle | Published once (or republished under the same name — see [app pointers](app-manifests.md#resolving-by-name-app-pointers)); inert between publishes | Runs continuously, or invoked per-command, like any normal program |
| Network capability | None directly — can only do what plain browser JS can do (fetch, WebSocket to some *other* server, etc.) | Full: `put`/`get`/`export`/`import` objects, `dial`, `announce`/`find_providers`, `publish_app_pointer`/`resolve_app_pointer`, gossipsub `publish`/`subscribe` |
| Analogy | A file on IPFS / a page on Netlify | A daemon, bot, or CLI tool that happens to use Canopee as its backend |
| Concrete example | Alice's portfolio site, [`spa-hosting-tutorial.md`](spa-hosting-tutorial.md)'s React app | `canopee-cli` itself, or [`crates/canopee-sdk/examples/app.rs`](../crates/canopee-sdk/examples/app.rs) |

## Where they meet

A published SPA and a `canopee-sdk` app aren't mutually exclusive — you can
build a React frontend (published via `app-manifest`) that's *meant* to be
the UI for a Canopee-native feature, e.g. a chat app, a file-sharing tool,
a collaborative doc. But the moment that frontend needs to actually call
`announce`, `subscribe` to a topic, or resolve an app pointer, it hits a
wall: **nothing bridges a browser tab to `canopee-sdk`'s Unix-socket
protocol.** `canopee-node`'s socket isn't reachable from a browser sandbox,
and there's no HTTP/WebSocket gateway in front of it today (unlike, say,
how `canopee open`'s own HTTP server exists purely to serve static files,
not to proxy the protocol).

In practice, today, that means:

- The *real* Canopee-facing logic — talking to peers, the DHT, pub/sub —
  has to live in a `canopee-sdk`-linked backend process, not in the
  published frontend's JS.
- The published static frontend can talk to that backend process some
  other way (e.g. it exposes its own regular HTTP/WebSocket API that the
  frontend calls with `fetch`/`WebSocket`, unrelated to Canopee's own
  protocol) — but at that point you're running an ordinary client-server
  app, with Canopee only involved on the backend half and as the static
  file host for the frontend half.
- There is currently no supported way for code running *inside* a
  published app's browser tab to directly drive `canopee-sdk` calls. If
  you want that, it's a real, unbuilt feature — something like a
  WebSocket/HTTP gateway in front of the node's Unix socket, exposing a
  safe subset of `NodeCommand`s to browser JS. Nothing in this repo
  attempts that yet; treat it as an open design question, not a documented
  limitation with a workaround.
- **This is only true for a plain browser tab.** A desktop shell like
  [Tauri](https://tauri.app) sidesteps the whole problem: its Rust backend
  can link `canopee-sdk`/`canopee-runtime` directly and expose it to the
  same app's frontend over Tauri's own IPC — no gateway needed, because
  the "backend" and the "UI" ship as one app instead of a webpage talking
  to a socket it can't reach. See
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) for a worked
  example.

## Which one do you want?

- **Just distributing a static site, portfolio, or SPA?** Use
  `app-manifest`. That's its entire job, and it already works — see
  [`app-manifests.md`](app-manifests.md) and, for React/Vite specifically,
  [`spa-hosting-tutorial.md`](spa-hosting-tutorial.md).
- **Building something that needs to actually talk to the Canopee
  network** — store/fetch objects, discover peers, pub/sub, resolve app
  pointers — **as part of its own logic, not just to be hosted by it**?
  Build it on [`canopee-sdk`](../crates/canopee-sdk/README.md) directly,
  the way `canopee-cli` itself does.
- **Want a real UI on top of `canopee-sdk`, with no separate node process
  for the user to run?** A desktop shell like Tauri sidesteps the "no
  bridge between a browser tab and canopee-sdk" problem entirely — its
  Rust backend links `canopee-sdk`/`canopee-runtime` directly and talks to
  its own frontend over Tauri's own IPC, no gateway needed. See
  [`tauri-chat-app-tutorial.md`](tauri-chat-app-tutorial.md) for a
  worked example (a chat app with an embedded node).
- **Want a browser-based UI for something built on `canopee-sdk`, with no
  desktop shell?** You'll still need your own bridge between the two
  today — see [Where they meet](#where-they-meet) above. This is the one
  combination that isn't already solved for you.
