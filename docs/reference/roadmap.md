# Roadmap

Published planning documents. Each is a decision doc: what's worth building
to close a gap, weighed honestly. The full texts live in
`docs/archive/legacy-docs/` for history; summaries live here.

## 1. Replacing a hosting provider

`roadmap-hosting-replacement.md` — "what it would take for Canopee to
replace a hosting provider."

- **Buildable, already scoped**: network-level content delivery (fetch +
  provider announcements already replicate content), the app-manifest
  publishing pipeline, and the LOOPBACK local serving already cover the
  hosting "serve my app" baseline.
- **Buildable, unstarted, genuinely hard**:
  - *HTTPS* (see the dedicated doc below),
  - *performance at scale* (swarm event throughput, provider-record churn,
    caching policy),
  - *operational tooling* (logging, metrics, diagnostics for remote nodes).
- **Not gaps — structural tensions**: uptime SLAs, and content
  moderation/takedown. These are tradeoffs inherent to a serverless model,
  not missing features. The doc states them plainly: in a P2P system nobody
  can promise five-nines, and nobody *can be compelled* to take content down
  either.

## 2. HTTPS/TLS for `canopee open`

`roadmap-https-tls.md` — a decision document about serving published apps
over TLS.

- Problem A: TLS **within the loopback boundary** (browser → local server) —
  cosmetic; loopback is reasonably served over plain HTTP, matching the
  localhost conventions of dev servers.
- Problem B: TLS for a site the **public** reaches — the real problem,
  because it requires a Web-PKI certificate, which needs a stable public
  hostname, which a swarm of NAT'd peers does not have.
- Options weighed: reverse proxy in front of `open` (cert owned by the
  proxy), TLS built into `serve` (and the cert problem beneath it), or
  avoiding WebPKI entirely (the Tor/I2P lesson: security *without* certs,
  at the cost of transport distinctness).
- **Recommendation**: keep loopback plain for the default flow; treat public
  TLS as a deployment concern handled by a reverse proxy on a stable public
  node — and keep the WebPKI alternative as the documented fallback.
- Explicitly not decided there: exact serving semantics, caching headers.

## 3. App ideas

`app-ideas.md` — what other apps are possible on Canopee, and which deserve
a tutorial.

- Already has a tutorial: peer-to-peer chat; static app publishing/hosting;
  app manifests + `canopee://` URIs; the Tauri SDK sample apps.
- Deserves a tutorial, not yet written: a blog/wiki with real version
  history; multi-device identity sync; a P2P bulletin board/forum.
- Interesting, not yet well-scoped: signed reputation/review systems;
  real-time voice/video calling.

## Cross-cutting

The capability layer (signed grants, [`capabilities.md`](capabilities.md))
is the foundation several of these build on: "who can read this channel /
this shared object / this app channel" is exactly what permission gating a
hosting replacement or a forum needs.