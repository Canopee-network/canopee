# Tutorial: hosting a real React (SPA) app, not just a static site

This is a hands-on, step-by-step guide to a specific gap: `canopee
app-manifest` already publishes *any* directory with a top-level
`index.html`, which happens to be exactly what `npm run build` produces for
a React app (Vite, Create React App, or Next's static export). Publishing
one today mostly works — but two rough edges specific to single-page apps
(SPAs) will bite you the moment the app does client-side routing or ships a
font/icon file. This tutorial walks through fixing both.

No code is given here — each step describes what to build, which existing
functions to hook into, and how to verify it worked before moving to the
next step. Read [`app-manifests.md`](app-manifests.md) first if you
haven't — this tutorial assumes you're comfortable with `publish_directory`,
`fetch_app`, and `serve`.

## The problem, concretely

Publish a React build and open it today:

```bash
cd my-react-app
npm run build                       # produces dist/ (Vite) or build/ (CRA)
canopee app-manifest ./dist alice-react-app
```

```bash
canopee open --owner <alice-id> --name alice-react-app
# Opening "alice-react-app" by canopee://identity/12D3KooW...alice
# Serving app at http://127.0.0.1:54291
```

Open `http://127.0.0.1:54291` in a browser — the landing page loads fine.
Now try either of these, both completely ordinary things a real user does:

1. **Click a link that uses React Router**, then hit refresh, or paste
   that URL into a new tab. E.g. the app renders `/about` client-side ($no$
   network request, just JS updating the DOM), but *loading* `/about`
   directly means the browser asks Canopee's server for `/about` as a
   fresh HTTP request. That path was never published — it doesn't exist in
   the app's `assets` map, only `/` does. The result is a raw `404`,
   because that's exactly what `handle_connection` in
   [`crates/canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs)
   does for any path not found in its `files` map: **every real static
   host (Netlify, Vercel, GitHub Pages, nginx `try_files`) handles this by
   falling back to `index.html` for unknown paths — Canopee's server
   doesn't yet.**
2. **The build includes a `.woff2` font, a `favicon.ico`, or a
   `.map` sourcemap file** — all completely normal Vite/CRA output.
   `guess_content_type` in the same file only recognizes
   `html/css/js/json/png/jpg/jpeg/svg`; everything else falls through to
   `application/octet-stream`. Fonts served with the wrong MIME type are
   silently ignored by some browsers, so part of the app's styling can
   quietly break with no error in sight.

We'll fix these in order: the content-type gap first (smaller, safer,
no ambiguity in what "correct" means), then SPA fallback routing (bigger,
requires a design decision about *when* to fall back).

## Step 0: orient yourself in the existing code

Read these before changing anything — all in
[`crates/canopee-cli/src/app.rs`](../crates/canopee-cli/src/app.rs):

- `publish_directory` — walks the built app directory and maps every file
  except `index.html` to `/<relative-path>` in `assets`. Notice it does
  **no filtering by extension** — anything in the build output directory
  gets published as-is, including `.map` files, `.txt` license files,
  whatever your bundler happened to emit.
- `fetch_app` — resolves a manifest into the in-memory
  `HashMap<String, Vec<u8>>` that `serve` hands out. Notice the key `"/"`
  is hardcoded for the entrypoint (`files.insert("/".to_string(), ...)`)
  — every other file is keyed by its published asset path exactly as
  `publish_directory` wrote it.
- `handle_connection` — parses the request line's path, looks it up
  *exactly* in `files`, and returns `404` on any miss. This exact-match
  lookup is what you'll be changing in Step 2.
- `guess_content_type` — a flat `match` on file extension. This is what
  you'll be extending in Step 1.

**Checkpoint:** run the two-node test from
[`app-manifests.md`](app-manifests.md#end-to-end-alice-publishes-pierre-opens-it-alice-republishes),
but publish a real Vite/CRA build instead of a hand-written `index.html`.
Confirm you can reproduce both problems above before writing any code —
you want to see the exact `404` and inspect the wrong `Content-Type` header
yourself (`curl -I http://127.0.0.1:<port>/some-font.woff2`) so you know
what "fixed" looks like.

## Step 1: extend `guess_content_type`

**Goal:** every file type a modern JS bundler commonly emits gets a
correct MIME type, not `application/octet-stream`.

**Where:** `guess_content_type` in `app.rs` — it's a pure function with no
async, no I/O, and an existing unit test (`serves_index_and_assets`) you
can pattern-match against for how to add your own.

**What to add:** work out, from an actual `dist`/`build` output of a real
React app, which extensions show up that aren't already handled. At
minimum expect to need:
- `woff`/`woff2`/`ttf`/`eot` (fonts)
- `ico` (favicons)
- `webp`/`gif`/`avif` (images beyond the existing `png`/`jpg`/`svg`)
- `map` (sourcemaps — these are JSON, but serving them as `application/json`
  vs. the more specific `application/json` is fine either way; the point is
  not falling through to `octet-stream`)
- `txt` (license files some bundlers emit alongside the build)

**Design question:** should an unrecognized extension keep defaulting to
`application/octet-stream`, or is there a better generic fallback? Think
about what a browser does with each: `octet-stream` typically triggers a
download prompt instead of rendering/using the file inline, which is
correct for something you genuinely don't recognize, but wrong for a font
file that just isn't in your `match` yet. This is the actual argument for
being thorough in this step rather than "good enough" — an incomplete list
doesn't just look incomplete, it produces silently broken pages.

**How to verify:** after extending the match, republish the same build
from Step 0's checkpoint and re-run `curl -I` against a font/icon file.
Confirm the `Content-Type` header now matches what a real static host
(check what Vercel/Netlify serve the same file as, in a browser's network
tab, if you want a ground truth) would send. Then load the app in an
actual browser and confirm whatever previously looked visually broken
(missing icon, fallback system font instead of the custom one) now
renders correctly.

## Step 2: SPA fallback routing

**Goal:** requesting any path not in the published `files` map serves
`index.html` instead of a `404` — the standard behavior every SPA host
provides, letting the client-side router take over and render the right
view once the JS loads.

**Where:** `handle_connection` in `app.rs` — specifically the line
`let body = files.get(&path);` and the `404 Not Found` branch right after
it.

**The core change:** when `files.get(&path)` misses, instead of
immediately returning `404`, fall back to `files.get("/")` (the
entrypoint) and serve *that* — with a `200`, not a `404` — so the browser
receives real HTML and the app's router (React Router, etc.) takes it from
there on the client side.

**Design questions to work through before coding — this is the part that
actually requires judgment, not just typing:**

- **Not every miss should fall back.** A request for `/style.css` that's
  genuinely missing (typo in the build, asset never got published) should
  probably still be a real `404` — falling back to `index.html` for *every*
  miss would make broken asset references invisible (you'd get a 200
  response full of HTML where you expected CSS, silently). Common
  approach: only fall back for paths that look like a route, not an
  asset — e.g. paths with no file extension in the last segment, or paths
  that don't match anything in a known "these are assets" set. Decide your
  heuristic and write down why you chose it.
- **What about a request for a real sub-path asset**, e.g.
  `/assets/index-a1b2c3.js`, when for whatever reason it's missing from
  `files`? Your heuristic from above should treat this as a genuine `404`,
  not fall back — walk through your logic against this exact case to make
  sure it does.
- **Does this change interact with Step 1?** Once you fall back to
  `index.html`, its `Content-Type` should be `text/html`, same as a direct
  `/` hit — confirm `guess_content_type` is being called with the right
  path (probably `"/"` or `"index.html"`, not the originally-requested
  route) so the fallback response's header doesn't lie about what's in the
  body.
- **Should the fallback only trigger for `GET` requests?** The existing
  server doesn't even parse the HTTP method today (look at how
  `handle_connection` reads the request line — it only pulls out the path,
  never checks `GET`/`POST`/etc.). Decide whether that's in scope for this
  tutorial or a separate concern; it's fine to explicitly punt on it, but
  say so rather than leaving it as an accidental gap.

**How to verify:** using the same published React build:
1. Load `/` directly — should still work exactly as before.
2. Client-side navigate to a route (e.g. click a nav link to `/about`) —
   should still work exactly as before (this was never broken, since it's
   pure client-side JS).
3. **Load `/about` directly** (paste the URL, or hit refresh while on that
   route) — this is the case that was broken. It should now return `200`
   with the app's `index.html`, and the app should render the `/about`
   view correctly once React Router picks up the URL client-side.
4. Request a deliberately-broken asset path, e.g. `/assets/does-not-exist.js`
   — confirm this **still** returns a real `404`, not a silent fallback to
   `index.html`. This is the test that actually proves your heuristic from
   the design questions above works, not just that you added a fallback at
   all.

## Step 3 (stretch): validate against a real build's asset base path

**Goal:** catch, with a clear error, the one common React build
misconfiguration that breaks Canopee publishing in a way that isn't
Canopee's fault: a non-root `base`/`publicPath` config.

**The problem:** Vite (`base` in `vite.config.js`) and CRA (`homepage` in
`package.json`) both support building an app for deployment under a
sub-path, e.g. `/my-app/`. When set, the build's `index.html` references
assets as `/my-app/assets/index-a1b2c3.js` — but `publish_directory` maps
files relative to the build directory root, so the manifest's `assets` key
would be `/assets/index-a1b2c3.js` (no `/my-app` prefix). The HTML and the
manifest disagree, and the browser requests a path Canopee never
published — indistinguishable, from the outside, from Step 2's problem,
but a completely different root cause (a build misconfiguration, not a
missing server feature).

**What to build:** after `publish_directory` finishes walking the
directory, parse the entrypoint's HTML (a simple string search for
`src="/` or `href="/` attributes is enough — you don't need a full HTML
parser for this) and cross-check that every absolute asset path it
references actually exists as a key in the `assets` map you just built.
If any don't match, fail `app-manifest` early with an error naming the
mismatched path and suggesting the user rebuild with `base: '/'` (Vite) or
remove `homepage` (CRA) — much better than a confusing 404 discovered
later, by someone else, after the app's already been shared.

**How to verify:** deliberately build a Vite app with `base: '/my-app/'` set,
try to publish it, and confirm you get a clear error at publish time
rather than a working-looking publish that silently 404s every asset once
opened.

## Design decisions (as implemented)

For anyone reading this after the code landed, here is what was chosen and
why:

- **Fallback heuristic:** a request only falls back to `/` when the last
  path segment has no file extension (no `.` in it) — i.e. it "looks like a
  route". Paths whose last segment *does* have an extension (`/style.css`,
  `/assets/index-a1b2c3.js`) never fall back; a genuinely missing asset is
  a real `404`, so a broken asset reference shows up as a visible error
  instead of a silent 200-of-HTML. Nested routes (`/users/42`) and query
  strings (`/users/42?tab=posts`, which `handle_connection` now strips
  before lookup) both work.
- **Fallback body's `Content-Type`:** is derived from the key actually
  served (`/`), so it's always `text/html` — derived from the served key,
  never the requested route.
- **Method handling:** explicitly punted — `handle_connection` still only
  reads the path element of the request line and ignores the HTTP method,
  exactly as before. Out of scope for this tutorial.
- **MIME fallback:** unrecognized extensions stay
  `application/octet-stream` (a browser's "this forces a download" default
  is the right behavior for something we genuinely don't know), but the
  mapping now covers everything a modern JS bundler realistically emits:
  `html/htm`, `css`, `js/mjs`, `json/map`, `txt`, `xml`, `wasm`,
  `png/jpg/jpeg/gif/webp/avif/svg`, `ico`, `woff/woff2/ttf/otf/eot`,
  `mp4/webm`, `mp3`.
- **Base-path validation:** `publish_directory` scans the entrypoint HTML
  for `src="/` and `href="/` attributes (plain string search, no HTML
  parser), cross-checks each absolute path against the `assets` map it just
  built, and fails `app-manifest` with an error naming every missing path
  and suggesting `base: '/'` (Vite) / removing `homepage` (CRA).
  Protocol-relative (`//…`) and full-URL (`https://…`) references are
  ignored, as are query strings and fragments.

## Verified end-to-end

Steps 1–3 landed together and were verified both by unit tests
(`cargo test -p canopee-cli`) and against a live two-node setup:

- Alice published a hand-built SPA directory: `index.html` referencing
  `/favicon.ico`, `/assets/app.css`, `/assets/app.js`; Pierre opened it via
  `canopee open --owner <alice-id> --name spa-demo --peer <alice-peer>`.
- `GET /` → `200 text/html`, the entrypoint.
- `GET /about` (a route never published) → `200 text/html`,
  the *same* entrypoint body — client-side router can now take over.
- `GET /users/42?tab=posts` → `200 text/html`, entrypoint.
- `GET /assets/app.js` → `200 application/javascript`;
  `/assets/app.css` → `200 text/css`; `/favicon.ico` → `200 image/x-icon`
  (all previously `application/octet-stream`).
- `GET /assets/nope.js` → `404` (missing *asset* stays an error).
- Publishing an entrypoint referencing an unpublished path
  (e.g. a Vite build made with `base: '/my-app/'`) fails `app-manifest`
  immediately, naming the offending paths.

## Summary checklist

- [x] Step 1 — `guess_content_type` covers fonts, icons, and other common
      bundler output, verified against a real build's `Content-Type`
      headers
- [x] Step 2 — unknown route-shaped paths fall back to `index.html` with a
      `200`, while genuinely missing assets still correctly `404` —
      verified with both a working client-side-routed page *and* a
      deliberately-broken asset path
- [x] Step 3 (stretch) — `app-manifest` catches a non-root build base path
      at publish time instead of failing silently later

By the end, publishing `npm run build`'s output should behave like
publishing to any other static host: direct links to any route work,
fonts/icons render correctly, and a common build misconfiguration is
caught immediately instead of shipping a broken deploy.
