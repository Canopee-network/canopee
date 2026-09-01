# Roadmap: HTTPS/TLS for `canopee open` — a decision document

This is a design decision document, not a tutorial. It answers one question
honestly: **what would it take for `canopee open` to serve a site over
HTTPS, and should it?** It follows the same pattern as the bootstrap
tutorial's Step 4 — a written design doc is a legitimate stopping point
here, and this one concludes with a specific, reasoned recommendation and
an explicit list of what is *not* decided.

Read [`roadmap-hosting-replacement.md`](roadmap-hosting-replacement.md)
first. It flags TLS as "the single biggest unstarted item on this list" and
"a real blocker, not polish" for the drop-in-replacement goal. This doc
takes that claim apart, because on inspection the problem splits into two
very different sub-problems that the roadmap's framing runs together.

## The current state, precisely

[`serve()`](../crates/canopee-cli/src/app.rs) binds a hand-rolled HTTP/1.1
server to `127.0.0.1:port` and prints `Serving app at http://<addr>`. It
speaks keep-alive, `Range`, `gzip`, `ETag`/`304`, and `HEAD` — a fully
functional *local* static file server. It is, deliberately, not a public
server: it binds loopback, serves content read from the local node's own
storage, and is reached by a browser running on the same machine as the
`canopee open` process.

That last point matters enormously for the TLS question, and it's the thing
the hosting-replacement framing glosses over.

## The two problems that "HTTPS" is really two of

### Problem A: TLS *within* the local loopback boundary

The common case today is: you run `canopee open` on your own machine, and a
browser tab on that same machine fetches `http://127.0.0.1:PORT/`. The
traffic never leaves the host. A man-in-the-middle here would require
already having local control of the machine — at which point the browser
could just read the served bytes directly, and (far more importantly)
[the node's whole identity is already on that machine](security-considerations.md),
including the signing key. **TLS adds essentially nothing to the localhost
threat model.**

The real reasons people hit "I want HTTPS for my local site" are almost
never confidentiality. They are:

- **Browser feature throttling.** Browsers degrade/withhold certain APIs
  (service workers, geolocation, and others) on insecure (HTTP)
  origins, even on localhost — though notably *most* of those are treated
  as "potentially trustworthy" when the origin is literally `127.0.0.1` or
  `localhost`, which is why a local SPA generally works today. The places
  this bites are edge cases, and worth enumerating rather than assuming.
- **Warnings/UX.** Some tooling and some users treat the "Not secure"
  indicator as a bug even when the origin is loopback.
- **Copy-paste friction.** An `http://localhost:PORT` URL is less
  "presentation-friendly" than a real `https://` URL, which matters for the
  "publish a portfolio and send someone a link" pitch.

None of these is solved in a satisfying P2P way for loopback. The standard
industry answer — used by Vite, `Caddy`, `mkcert`, and every local-dev
web server that offers "HTTPS for free" — is **a locally-trusted
certificate authority** (`mkcert` installs a private root CA into the
OS/browser trust store and issues certs for `localhost` from it). This is
elegant, requires zero external infrastructure, and is exactly how local
TLS is supposed to work.

But note what it does *not* give you: it is not a certificate the *public*
would trust. It is scoped to "this one machine trusts this one root CA."
So it solves Problem A cleanly and does nothing for Problem B.

### Problem B: TLS for a site the *public* reaches — the cert problem the roadmap is really about

This is where the hosting-replacement goal lives, and it is the genuinely
hard case. A real public deployment needs a certificate that *any* browser
on the internet will validate against the WebPKI's built-in root stores.
There is no in-host CA trick for that; you must get a cert from a public CA
(or ride on one, as below).

The blocker, stated precisely in the roadmap: **certificate issuance
assumes identity == a DNS name you control.** Canopee's addressing is
`canopee://<owner>/<name>` — identity-based, an Ed25519 public key, not a
domain. No public CA will issue a certificate attesting "this TLS key
belongs to this Ed25519 public key," because the WebPKI has no vocabulary
for that. The CA/browser trust infrastructure is built on DNS names and,
increasingly, CAA/DNS-holes. It has no concept of a content-addressed,
key-derived namespace. **The WebPKI cannot natively express "this
canopee://identity is authentic," and no amount of effort on Canopee's
side changes that — the other side of the handshake is a browser that only
understands DNS + CAs.**

This is not a thing Canopee is missing. It is a structural mismatch between
"identity that is a public key" and "identity that is a domain," and it is
exactly why **[Tor](https://www.torproject.org) and [I2P](https://geti2p.net)
deliberately do not do browser-visible HTTPS the normal way** (the roadmap
already notes this). Those systems face the same wall and route around it
rather than fight it. That route-around is the transferable lesson.

## The options, honestly weighed

### Option 1 — Reverse proxy in front of `open` (cert owned by the proxy)

Put nginx/Caddy in front, let *it* hold a normal WebPKI cert for a domain
*you* own, and have it forward to `canopee open`'s loopback server.

- **Pros:** zero changes to `canopee-cli`; you get a real, public-trusted
  cert; it works today with no new Canopee code.
- **Cons:** you now need a server with a domain and a cert — which is
  precisely the centralized hosting infrastructure the whole project
  exists to avoid. It works, but it stops being "ownerless P2P hosting" at
  the moment you point a VPS at it. Best understood as "Canopee as a fast
  CDN-origin backend to a normal static host," which is a different pitch.
- **Honest verdict:** this is really the `custom-domain bridge` item from
  the operational-tooling bucket, not a TLS feature. It solves Problem B by
  capitulating on Problem B's framing — you stop serving Canopee's address
  and serve a domain instead.

### Option 2 — TLS built into `serve` (and the cert problem beneath it)

Add a TLS termination layer inside `serve()` (e.g. `rustls`), as the
roadmap's "Build TLS into serve itself" path suggests. This is a real,
buildable engineering item **for Problem A only** (pair it with a
locally-trusted root CA à la `mkcert`), and it is straightforward. But the
moment you ask "and where does the public cert come from for Problem B,"
you are back at the wall above — a hand-rolled TLS server still needs a
certificate, and `canopee://owner/name` still isn't a domain. **Building
TLS in does not touch the hard 80% of the problem**; it adds an HTTP/1.1 →
TLS refactor of a hand-rolled server for the easy 20% it solves locally.

### Option 3 — Avoid the WebPKI entirely (the Tor/I2P lesson)

Don't try to make browsers trust a `canopee://` origin over the WebPKI.
Instead, either:

- **Route through a canonically-hosting path that *is* WebPKI-trusted** —
  i.e. Option 1 again; or
- **Ship a browser that understands `canopee://` natively.** This is the
  "canopee://-aware desktop browser" idea from
  [`publishing-vs-building-apps.md`](publishing-vs-building-apps.md)'s
  operational-tooling section. Such a browser doesn't need a CA to trust
  `canopee://owner/name` — it maps the owner to the object directly and
  gates trust on the Ed25519 signature over everything, the same way Tor
  Browser gates onion-service identity on the onion address itself. HTTPS
  per-connection can still be used inside, but certificate *trust* is
  solved by the browser knowing what a Canopee identity is, not by the
  WebPKI. This is the only option that keeps the decentralized value
  proposition intact for Problem B — at the cost of not working in a
  stock browser.

### The honest synthesis

- **Problem A (local loopback) is cheap and self-contained:** add optional
  TLS to `serve()` using a locally-installed private root CA, exactly like
  `mkcert`. Low risk, no external infra, and it closes the "Not secure" /
  feature-throttling/UX gaps. This is worth doing on its own and is
  orthogonal to the big question.
- **Problem B (public trust) is not a TLS-implementation problem, it's a
  WebPKI-structural mismatch.** The only paths through it that preserve
  Canopee's decentralization are Option 3-style (a native `canopee://`
  browser, or accepting the roadmap's own conclusion that full
  browser-visible public HTTPS for a key-based address is out of reach in a
  stock browser). Option 2 doesn't reach Problem B at all; Option 1 reaches
  it by giving up the decentralized premise.

## Recommendation

Do **Option 2 scoped strictly to Problem A** as an incremental, opt-in
improvement, and **do not attempt to solve Problem B in a stock browser** —
document it as a structural tension rather than a blocker to fix.

Concretely, the recommended next piece of work:

1. Optionally wrap `serve()` in TLS using `rustls` + a locally-trusted
   root CA, gated behind an explicit flag (e.g. `--tls` / a config knob),
   off by default so existing behavior is unchanged. The cert is minted for
   `localhost`/`127.0.0.1` from a CA the operator installs into their own
   trust store (the `mkcert` model). This is a contained refactor of the
   hand-rolled server's transport layer, and it closes the real,
   reachable "HTTPS for my local site" gap.
2. Update `security-considerations.md` and this doc to state plainly that
   publicly-trusted HTTPS for a `canopee://` origin is **not achievable in
   a stock browser without either (a) a domain (giving up decentralized
   addressing for that deployment) or (b) a WebPKI-independent browser**
   — and that this is a deliberate structural consequence of identity-based
   addressing, mirroring Tor/I2P, not an oversight.

## Explicitly not decided here

- The exact TLS implementation (rustls vs. a `tokio-rustls`/`axum`-style
  rewrite of the hand-rolled server) — to be chosen if/when the localhost
  TLS work is picked up.
- Whether a native `canopee://` browser is ever built (Option 3) — a large,
  separate undertaking, noted here only as *the* structurally-sound answer
  to Problem B, mirroring Tor/I2P.
- Whether a custom-domain / reverse-proxy bridge (Option 1) is desirable
  for specific deployments — that's the operational-tooling decision, and
  for "replace a commercial host with a drop-in solution" it is the only
  realistic path, at the cost of the decentralized premise.

## Summary

`canopee open` already serves its intended local audience across
`127.0.0.1`. Adding HTTPS there is a small, self-contained win (Problem A,
via a local root CA) that doesn't depend on any of the hard questions.
Making a `canopee://` origin trustworthy to *public* browsers (Problem B)
is not an implementation gap Canopee can close from its own side — it is a
structural mismatch between key-based identity and the DNS-based WebPKI,
and the honest move is to say so, not to bolt TLS onto a server and pretend
the certificate problem is thereby solved.
