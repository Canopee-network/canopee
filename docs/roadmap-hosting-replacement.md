# Roadmap: what it would take for Canopee to replace a hosting provider

This is a gap analysis, not a tutorial — it doesn't walk through building
anything step by step (see the tutorials below for that). It exists to
answer one question honestly: **what's actually missing between "a working
reference implementation" and "something you'd point a real site at
instead of Netlify/Vercel/a VPS"?**

The gaps split into three kinds, and telling them apart matters:

1. **Buildable, and already scoped** — tutorials exist; this is
   incremental engineering work.
2. **Buildable, but unstarted and genuinely hard** — real engineering,
   not yet designed.
3. **Not gaps at all — tensions with what makes Canopee decentralized in
   the first place.** Closing these would mean giving up the thing that
   makes Canopee different from a hosting provider, not catching up to
   one.

Read [`publishing-vs-building-apps.md`](publishing-vs-building-apps.md)
first if you haven't — this doc is specifically about the "published app"
side (`app-manifest`/`open`), since that's the half of Canopee that's
directly comparable to a hosting provider at all.

## 1. Buildable, and already scoped

These have dedicated tutorials. Doing them gets you a materially more
production-viable static host; none of them require rethinking Canopee's
architecture.

| Gap | Why it matters for "replaces hosting" | Where |
|---|---|---|
| Apps only reachable while the publisher is online | A hosting provider doesn't go down when *you* turn your laptop off. Every other node that's fetched an app should be able to keep serving it. | [`p2p-app-caching-tutorial.md`](p2p-app-caching-tutorial.md) |
| No path into the network without a hand-pasted multiaddr | A hosting replacement can't require every new user to already know a peer's address before anything works. | [`bootstrap-nodes-tutorial.md`](bootstrap-nodes-tutorial.md) |
| SPA routing 404s on direct links/refresh; missing MIME types | Real sites — not just hand-written single pages — need this to work at all, let alone well. | [`spa-hosting-tutorial.md`](spa-hosting-tutorial.md) |

Do these three and you have most of what "a working, resilient static
host" needs. What's left is harder.

## 2. Buildable, unstarted, genuinely hard

No tutorial for these yet — they're real engineering, not polish, and each
one is its own design problem before it's an implementation problem.

### HTTPS

`serve()` in [`crates/canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs)
is plain HTTP on `127.0.0.1`. No real deployment serves a public-facing
site without TLS today — browsers actively warn on/degrade plain HTTP
(no service workers, geolocation, many modern APIs). Two paths, and they
have different downstream consequences:

- **Terminate TLS in front of `open`** — a reverse proxy (nginx, Caddy)
  sitting in front of the local HTTP server. Simplest to build (zero
  changes to `canopee-cli`), but reintroduces exactly the kind of
  centralized infrastructure a P2P host is supposed to avoid — you're back
  to needing *a* server with *a* certificate, just for TLS.
- **Build TLS into `serve` itself.** Consistent with "no dependency on
  anything but Canopee," but runs straight into the certificate problem
  below.

Either way, you hit: **certificate issuance assumes a DNS name and a CA**
(Let's Encrypt, the standard free option, validates ownership of a domain
you control). Canopee's addressing is `canopee://<owner>/<name>` —
identity-based, not DNS-based. There's no existing CA that issues a
certificate for "this Ed25519 public key," and building a P2P-native trust
model for TLS is its own significant design problem (closer to how
Tor/I2P avoid this entirely by not doing browser-visible HTTPS the normal
way at all). This is the single biggest unstarted item on this list.

### Performance at scale

`serve()` reads a whole file into memory and writes it in one `write_all`
call — fine for a portfolio, not fine for a video or a large dataset.
Done in [`canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs), in the
order you hit them:
- **Keep-alive persistent connections** — multiple requests per connection
  (honoring the client's `Connection` header, with an idle timeout), instead
  of `Connection: close` on every response.
- **Range requests** (`Range`/`Content-Range`) — single-byte-range 206
  responses, 416 for unsatisfiable ranges, `Accept-Ranges: bytes` advertised;
  needed for video seeking, resumable downloads, and large-file behavior.
- **Compression** (`gzip` `Content-Encoding`) — text assets (JS/CSS HTML)
  served gzipped when the client accepts it, with `Vary: Accept-Encoding`;
  binary formats skipped.
- **`ETag` + 304 conditional revalidation** — the ETag is the object's
  SHA-256 content identity, so browser caching and object identity share one
  value; stale-condition GETs answer 304 with no body.
- **`HEAD` without a body** — headers (`Content-Length`, etc.) still match a
  GET. Non-GET/non-HEAD methods are still answered like GET (read-only host).

Still open:
- **HTTP/2 or better** — the server is HTTP/1.1 (now keep-alive, but no
  multiplexing / server push). A real HTTP stack (hyper/h2) is a significant
  refactor of the hand-rolled server; documented, not yet undertaken.
- **`brotli`** — browsers benefit modestly over gzip for text; adds a pure
  native dependency; deferred in favor of gzip.
- **Multi-range and `If-Range`** — out of scope, explicitly allowed to be
  ignored per RFC 7233 §3.1 (multi-range falls back to a full 200).
- **Edge/CDN-style caching** — a hosting provider puts your content
  physically close to every visitor. Canopee's caching tutorial gets you
  *replication* (more nodes have a copy), which is a meaningfully
  different property from *geographic proximity to a specific
  requester* — replication doesn't by itself make `open` pick a
  geographically close provider over a far one. That'd be a new piece of
  logic in `resolve_peer`/`get_or_fetch`, not something caching gives you
  automatically.

### Operational tooling

No access logs, no analytics, no custom domain support (`canopee://` isn't
`yoursite.com` — there's no bridge between Canopee's addressing and DNS,
and building one means either a browser extension, a `canopee://`-aware
desktop browser (conceptually: an app that embeds a node, resolves
`canopee://owner/name` addresses natively, and optionally bridges a
curated subset of `canopee-sdk` calls to page JS — see
[`publishing-vs-building-apps.md`](publishing-vs-building-apps.md) for the
underlying gap this would close), or a gateway server translating
`https://yoursite.com` requests into Canopee `open` calls — the last
option again reintroduces a centralized server), no CI/build-pipeline
integration (`app-manifest ./dist` is a manual step; a real deploy story
wants "push to a branch, it publishes automatically"), and no rollback
beyond "you happen to still have an older manifest id" — there's no
built-in history/versioning on top of `AppPointerRecord`, which only ever
tracks the *latest* pointer, not a log of previous ones.

## 3. Not gaps — structural tensions

These aren't things to build. Closing them would mean Canopee stops being
decentralized, which defeats the point of using it instead of a hosting
provider in the first place. Worth naming explicitly so "why hasn't this
been fixed" has an honest answer: it's not an oversight.

### Uptime SLAs

A hosting provider's business model is "we are contractually on the hook
if your site goes down." Canopee's caching tutorial gets you *statistical*
resilience — more copies, more providers, lower odds any single node's
outage takes your app down — but nobody is accountable for it the way a
company answering a support ticket is. This is inherent to a volunteer
P2P network, not a missing feature. If you need a contractual SLA, you
need a company standing behind the infrastructure, which is precisely the
thing decentralization is opting out of.

### Content moderation / takedown

A hosting provider can and does remove content on legal request, abuse
reports, ToS violations. A Kademlia DHT has no such lever by design — no
central party controls what gets announced, cached, or resolved. This cuts
both ways: it's the entire point for someone who wants censorship
resistance, and a genuine liability for anyone hoping to run something
that needs a moderation story (a hosting provider protects *you* from
liability for what your users publish; Canopee, as built, does not and
structurally can't in its current form).

### The actual tension, stated plainly

**"Replaces your hosting provider"** and **"fully decentralized, no
operator accountable for anything"** pull in opposite directions. A
hosting provider's value proposition largely *is* having someone
accountable — for uptime, for TLS certs, for taking down illegal content,
for support when something breaks. Canopee's value proposition — no
platform to depend on, cryptographic rather than custodial ownership,
names you sign yourself instead of registering with anyone — is
explicitly *not* having that party. You can move along this spectrum — e.g. adding
opt-in "sponsor a node with better uptime and support" services on top of
the base protocol — but you can't fully close this gap without
reintroducing the exact thing Canopee exists to avoid. This isn't a bug to
fix; it's a design tradeoff a "hosting replacement" pitch has to be honest
about rather than paper over.

## Where this leaves things

If the goal is "a genuinely more resilient, ownerless way to publish
static sites, with the caveats stated up front" — sections 1 and most of
2 get you there, and 1 already has tutorials ready to implement.

If the goal is "drop-in replacement for a commercial host, indistinguishable
to an end user" — HTTPS and custom domains (section 2) are real blockers,
not polish, and section 3's tensions mean some things (SLAs, takedown)
will never fully match a centralized host's offering without compromising
what makes Canopee worth using in the first place.
