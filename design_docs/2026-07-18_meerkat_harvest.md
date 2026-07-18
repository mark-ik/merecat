# The meerkat harvest: gems taken, gems noted, chaff named

2026-07-18, with Mark ("settings, then meerkat harvest and passing the torch
formally"). The sweep before the funeral: meerkat's 174 source files (61.5k
LOC) read for what the surviving ecosystem should TAKE (port now), NOTE
(backlog with a named home), or LEAVE (superseded or deliberately not
carried). The obviation doctrine held throughout — meerkat is a donor read
for technique, never copied wholesale — so most rows below are notes, not
ports. This doc is the deletion pass's receipt that nothing valuable dies
with the crate.

## Taken (ported at the harvest)

| Gem | From | To | Why |
| --- | --- | --- | --- |
| **Sprite collider-hull tracer** (`trace_sprite_hull`: grid sampler + monotone-chain convex hull + deviation-decimating simplify) | `sprite_import.rs` | **`mere::canvas::sprite_hull`** (+ merecat's drop lane wires it: `SetNodeSprite` now carries the hull) | Completes the file-drop lane's honest gap — a dropped sprite collides at its picture. Promoted to canvas, not merecat, so every host shares one tracer (the ecosystem-over-app steer). |
| **Inspector row semantics** | `inspector.rs` | merecat `inspector_view.rs` | Already taken at rung 5 slice D (re-cut, not copied: live-session `inspect()` instead of the body re-parse). |
| **Scenario/self-drive vocabulary** | `scenario/runner.rs` | merecat `scenario.rs` + genet-probe | Already taken as a re-derivation ("the vocabulary returns as Action-driven automation"); genet-probe generalized it ecosystem-wide. |
| **Behavioral test corpus as spec** | `agent_harness/tests.rs` (2,216 LOC), `tests.rs` (807) | This doc + the deletion-matrix receipts | Read as the checklist behind the matrix rows; the receipts (18 scenarios, 70+ unit tests) are their merecat successors. Nothing further to port — the harness itself is superseded by scenario + probe. |

## Noted (backlog, each with its named home)

Capabilities meerkat ships that merecat does not yet. None block the funeral
(none are deletion-matrix rows); each is recorded so its future implementer
reads the donor file BEFORE the crate leaves the tree — after deletion, `git
log` archaeology on mere's history still reaches them.

- **The focus card** (`card/`, `render/cards.rs` — 962 LOC): the summonable
  node-focus card, meerkat's third representation (node / gnode / card).
  Home ruled 2026-07-18 (Mark): cards return **only as cambium primitives** —
  a catalog `card` component any genet app composes, never a merecat-special
  rebuild. Meerkat's geometry + summon semantics are the technique read for
  that catalog entry when a consumer pulls it.
- **Command registry vocabulary** (`command.rs`, 603): ids, categories,
  binding declarations — richer than merecat's flat `palette_actions()`.
  Home: merecat's palette when it grows categories/keybinding surfaces (the
  architecture plan already earmarked this reread).
- **Settings pages** (`settings_lane/` 4 files ~1.4k, `settings_node.rs`,
  `settings_pane_view.rs`): the settings-AS-NODES design is ruled a **bust**
  (2026-07-18, Mark: `settings://` nodes bred "node settings of a settings
  node type" soup — do not resurrect it), and with it the PELT settings
  tiles it rendered through. The surviving shape is the retargeting PANE
  (merecat's Apparatus already works this way: sync to the selected node).
  With the reclaimable inventory absorbed pane-side, **nothing of the pelt
  settings machinery needs porting — it dies with meerkat entirely.** The
  harvestable remainder is only the PAGE CATALOG — the inventory of what was
  settable (engine prefs, retention, shellbar edge, script permissions) — as
  the checklist for pane-shaped settings surfaces to grow through.
- **Web clip** (`web_clip.rs`): page selection → stored node content. Home:
  merecat content lane backlog.
- **In-page find** (`find.rs`, `find_worker.rs`): the content-search lane.
  Home: merecat content lane backlog (genet-side `links()`/text APIs are the
  substrate now).
- **Theming lane** (`theme_sheets.rs` 639, `theme_store.rs`, `theme_edit.rs`,
  `tile_theme.rs`, `doc_style.rs`): palette-derived per-content themes.
  Home: rung 8 (register-theme/tinct); genet's livery is the future engine.
- **Export** (`export.rs`): graph/session export profiles. Home: merecat rung
  8; the deletion/retention plan names export-profile settings.
- **Wallet pairing** (`wallet_pairing.rs`, 869): personae pairing UX. Home:
  the comms rung (murm/moot posture) — UI to re-derive over personae, not
  copy.
- **Constellation actor pool** (`constellation/`, ~1.5k): per-node actor
  supervision + drain — Steward's data source. Home: read when merecat's
  Steward grows real rows (its status-port shape is the honest reference).
- **Graph delta log** (`graph_delta_log.rs`, 857): attributable graph-change
  diagnostics. Home: mere-side (chartulary's journal is the successor
  substrate; this file is the UX read on it).
- **Gnode pool + partitioned raster** (`window_view/gnode_pool.rs` 920,
  `render/paint.rs` 708): the abs-pos node-children pool technique and the
  `ChromeRasterPlan::Partitioned` miniature that presaged the forest dom's
  per-subtree layout. Superseded by canvas + forest dom, but the pool's
  batching tricks are worth a read if canvas node-body perf ever bites.
- **Knot completion / infer host / ingest / content affinity**
  (`knot_completion.rs`, `infer_host.rs`, `ingest.rs`,
  `content_affinity.rs`): the intel lane glue (vates/sibylla consumers).
  Home: rung 8 intel port; the affinity heuristics are the gem.
- **Idle forgetting + snapshot refresh** (`app_handler/idle_*.rs`): memory
  policy driven by idle time (alembic B5's by-session eviction consumer).
  Home: merecat once alembic surfaces land.
- **IME nuances** (`ime.rs`, `input/text_input.rs` 686): merecat's IME is
  landed; meerkat's file carries extra edge-cases (surrogate handling,
  composition-cancel) worth a diff-read if IME bugs surface.

## Left (superseded or deliberately not carried — the plans already said so)

- `command_drain.rs` (1,237) + `shell_eval.rs` (951): the imperative
  crossroads the Action spine exists to prevent. Doctrine 2's origin story.
- `sync.rs`: the peer-runtime plan's Phase F deletes this wiring by name
  (operation verification / insertion callbacks must not live app-side).
- `views/`, `window_view/`, `frame_view.rs`, `genet_render.rs`,
  `render/`: the retired Masonry-era + hand-rolled render paths; cambium +
  the layered present + forest dom are the successors.
- `frame_a11y*.rs`, `genet_a11y.rs`, `a11y_bridge`: merecat's stitched
  projection (rung 5 slice F) is the successor shape; the OS-adapter push
  neither app landed stays a named follow-on.
- `shell_new.rs` (714), `app_handler/`, `input/` (the ~2.5k dispatch tree):
  merecat's shell + surface plan re-derived these smaller.
- `main.rs` 85-module sprawl: the founding anti-goal.
- `meerkat-browser-worker`: the compat lane went to genet as `verso-tile`;
  the worker process dies with the app.

## Pane taxonomy REVISED at the harvest (2026-07-18, with Mark)

Atomic facets landing (chartulary's `facet.rs`/`content_class.rs`, same day)
re-charters three panes. This supersedes the 2026-07-10 taxonomy's
apparatus/steward split ("apparatus splits its natures"):

- **Apparatus = the graph-object facet analyzer.** Graph OBJECT metadata —
  facets, classifications, tags, provenance — analyzed and edited, for the
  selected object (the retargeting-pane pattern, never settings-as-nodes).
  The first consumer of atomic facets. Object-scoped handling controls (the
  viewer override that landed today) live here as its first editable rows;
  the pane grows facet analysis around them.
- **Inspector = content + content-metadata analysis and CLIPPING.** The
  parse/trust/structure read it has, plus the clip affordance (meerkat's
  `web_clip.rs` is the donor read) — content in, content examined, content
  clipped.
- **Steward eats sync AND async status.** All operational readouts — fetch
  actors, background work, AND the comms/sync rows the old taxonomy split
  out — one status pane, honest-empty until each lane lands.
- **Settings (app-level) are none of the three.** Distinct surface, distinct
  pane, pane-shaped, later; the page catalog above is its checklist.

## The funeral rite (the deletion pass this doc gates)

1. mere: remove `crates/meerkat` + `crates/meerkat-browser-worker` from the
   workspace and disk; drop their workspace-dep entries.
2. mere: `frisket` relocates to merecat (the 2026-07-14 decision — its
   `PaneContent` names merecat's panes); `session_runtime::frisket_store`
   goes with it; session-runtime drops its frisket dep.
3. mere facade: **unchanged** — platen + workbench STAY library (the
   2026-07-18 composition-domain-model decision reversed the boundary-pass
   move; isometry/woodshed are the prospective consumers).
4. merecat: frisket becomes a workspace member crate (same repo, so no local
   paths in committed manifests); the git dep on mere.git's frisket drops.
5. Docs: the founding doc's done-condition 2 stamps met; the obviation
   ladder closes; the torch passes.
