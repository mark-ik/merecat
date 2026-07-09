# Merecat founding

2026-07-08. Merecat is the promotion target for mere's `meerkat` app crate,
and the other half of a role swap: **mere becomes a library** (the lake: a
graph over semantically related content) and **merecat becomes its consumer**
(the reference host app, in its own repo, like every other promoted crate's
consumers).

## Why the rename

Too many apps, companies, and services already use "meerkat" (the dead
livestreaming app, Compare-the-Market's finance app, meerkat-software.com,
others). "Merecat" makes the pun explicit and load-bearing: meerkat is Dutch
*meer* + *kat*, lake-cat, so merecat is the English calque, and *mere* (a
lake) is the library it lives on. Known prior occupant: troglobit's `merecat`
embedded httpd (thttpd fork, BSD). Different domain; Mark is unbothered.
`merecat` was free on crates.io at founding.

## Target shape

- **mere** = the composition tier over the promoted families: kernel graph +
  domain vocabulary (gloss, apparatus, card, roster), eidetic persistence
  assembly over muniment/codicil, forme, and the composition seams where the
  host injects murm's endpoint and moot's store. Local-first in the strict
  sense: fully functional offline, no p2p deps in core. Mere has no window,
  engine, GPU, fetch, or browser-lifecycle knowledge.
- **merecat** = this repo. The app: windows, panes, input, render hosts,
  chrome, settings, session, **and the presentation surfaces**: orrery
  (graph canvas scene host), platen with its arrangements/cartography
  satellites (composition/projection), and the verso family (verso,
  verso-api, verso-scry, verso-serval, browser-worker: the engine
  multiplexer, the heart of the web lane). Amended 2026-07-09 per the
  [mere/merecat boundary pass plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-07-09_mere_merecat_boundary_pass_plan.md):
  the earlier shape kept orrery/platen in mere; they are application
  surfaces, and the `mere` facade's re-exports of them are compatibility
  scaffolding for the in-workspace host, not the library boundary. Its
  Cargo.toml should read like the ecosystem map: mere, personae, murm, moot,
  serval. A direct dep on something mere composes is the seam leaking.

## Sequencing (seam first, split second)

1. **G5 re-base defines mere-core as the library.** Done-condition: mere's
   lib crates sit over the chartulary family and meerkat depends only on
   their public APIs.
2. **Enforce the seam in-workspace.** Library-weight code moves out of the
   app bin down through the seam (graph_delta_log dissolves into
   chartulary/codicil; wallet_pairing goes to personae; the agent-harness +
   observability + a11y-projection cluster becomes promotable; scrying_host
   folds toward verso-scry; fetch/cookies toward the crawl lane). No
   `pub(crate)` reach-ins, no kernel hooks wired from the bin.
3. **Port meerkat → merecat once the seam holds** through normal work
   (observable signal: meerkat changes stop requiring same-day mere
   changes). Strip MPL headers on the way; this repo is MIT/Apache,
   edition 2024. Mere re-bases last, per standing doctrine.

Until step 3 the crate is a name-reserving placeholder. Murm/moot promotion
proceeds independently on its own purity gates (moot: no sockets; murm: no
store-of-record) and neither waits on the other.

## Current extraction state

The first in-workspace seam landed on 2026-07-09: mere now has a `mere`
library façade for graph truth and the composition tier (`kernel`,
`linked-data`, `graphlets`, `glossary`, `orrery`, `forme`, `platen`, and the
graph-domain vocabulary). The still-in-workspace host reaches those through
that façade rather than declaring the constituent crates directly. The façade
keeps SPARQL query support opt-in and exposes kernel fixtures only for tests.

This is deliberately not the port. The host still owns and directly depends on
its app-local chrome, rendering, session, actor, and browser-runtime lanes; it
must build from this repo against a branch dependency on `mere` before the
application moves here.

**First vertical slice landed 2026-07-09**: merecat builds and runs from this
repo against `mere` as a branch dep (git sibling + gitignored local patches,
plus the `[patch.crates-io]` entries restated from mere's workspace manifest —
a standalone consumer must carry those itself). The bin is a thin winit shell
hosting the window-agnostic `mere::orrery::Orrery` content-root: an address
argument mints its node in a fresh mere graph (`Orrery::visit`) and the canvas
renders it through serval-winit-host, with physics on an armillary actor.
Nothing was copied from meerkat's shell; orrery's demo-scene catalog stayed
behind. Headed receipt: `testing/merecat/images/2026-07-09_first_vertical_slice.png`.
Done-condition 1 of three is met; the next slices are the verso-api browser
lane (open address -> live content on the node) and session persistence.

**Second slice, same day — the web lane's first breath**: the seed address now
FETCHES. mere's `fetch` actor (armillary thread, netfetcher/errand + cookie
jar) is consumed as an individual crate from mere's workspace (it is app-side
material that moves here with the port); the page outcome stamps the response
Content-Type as the node's MIME hint and, for HTML, the static-parse
`<title>` (serval-extract) onto the node — the canvas caption flips from the
host fallback to the real title (`example.com` -> `Example Domain`,
receipt `testing/merecat/images/2026-07-09_fetch_title_enrichment.png`).
Supporting seam work in mere: orrery gained `set_node_title` (rebuilds the
caption pool) and `set_node_mime_hint` (metadata-only), mirroring its favicon
stamp; and the founded-family sibling deps (chartulary/stemma/numen/codicil/
muniment) converted from `../..` path deps to branch-tracked git deps, so
mere.git is consumable per-crate by standalone hosts (mere 8f338ff — clean
non-local resolution needs that commit pushed). Next: favicon-on-node, live
content rendering (verso lane), session persistence.

## Done-conditions

- merecat builds and runs from this repo against mere as a dependency
  (branch dep, not a local path, per workspace convention).
- mere's workspace no longer contains an app bin.
- The meerkat name survives only in mere's git history.
