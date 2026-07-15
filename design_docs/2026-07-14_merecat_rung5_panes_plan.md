# Merecat rung 5: panes

2026-07-14. Scopes rung 5 of the obviation ladder in
[2026-07-10_merecat_architecture_plan.md](./2026-07-10_merecat_architecture_plan.md).
The ladder's one-line gate for this rung ("platen's pane model") is wrong in
both halves, so this plan restates the gate before it slices the work.

## The correction

The ladder says rung 5 gates on "the surface composition + focus routing seam
(born minimal at rung 3) plus platen's pane model". Neither exists.

**Platen is not a pane model.** Every `Workbench` mutator takes a
`forme::GraphMemberId`, which is a type alias for the kernel `Node` UUID
(`platen/src/workbench.rs:170` `open_tile(member: GraphMemberId)`; also
`open_stack`, `close_tile`, `activate`, `split_beside_axis`). Its `Pane` type is
`pub(super)` and never exported. There is no `PaneId`; slots are addressed
positionally by `path: &[usize]`. A platen tile can only ever be a graph node, so
`Workbench` has no way to say "a gloss pane". Platen is a **node-tiling** model,
and a good one: 2,627 LOC, 50 tests green, a canonical
`(Arrangement, TreeGeometry)` persistence pair with `canonical_roundtrips()`
debug-asserted on every persist and a 600-seed random-gesture fuzz test.

**The pane model already exists, and it is `frisket`.**
`mere/crates/shell/frisket` (1,249 LOC, 14 tests): `PaneId(u64)`, a `PaneContent`
enum with a variant for every rung-5 pane, a `PaneNode` split tree, `FrisketLayout`,
and serde persistence that `session_runtime::frisket_store` already writes to
`frame.json`. Deps are accesskit, serde, tracing, uuid, uxtree only. Host-neutral,
geometry-free, and already compiled into merecat's dependency graph.

**The two nest rather than compete.** `PaneContent::Workbench` is one frisket leaf
whose content is a platen `Workbench`. Three tiers, not one:

```text
frisket    pane-ness: kinds, ids, split tree, frame.json    (mere/crates/shell/frisket)
platen     node-tiling INSIDE the workbench pane leaf         (mere/crates/platen)
surface    rects, z-order, focus routing, hit-test            (does not exist; slice A)
```

**The surface seam was never born.** `Shell::render` binds one
`ExternalTexturePlacement::new([0.0, 0.0, w, h])` and reuses it for all three
layers. There are no surface ids, no rects, no z-order. Focus routing is the
boolean `self.app.omnibar.open`, branched at three sites; continuous gestures
(CursorMoved, MouseWheel) do not consult even that and go unconditionally to the
canvas. So rung 5's first act is to build the thing the ladder books as supply.

The renderer is the one piece genuinely blocked, and the ladder never names it.
Platen emits fractions, never rects (`projection_geometry.rs`: "semantic geometry,
not pixels"). `platen-view` does not exist and is not pending; it was a real crate,
deleted 2026-06-15 as superseded by pelt. Platen's README still advertises it,
which is doc rot rather than a roadmap.

## The pane taxonomy

Decided 2026-07-10 with Mark, recorded in mere's boundary-pass plan. This plan does
not reopen it:

> mere owns what a pane says when it speaks graph or persistence truth, merecat
> owns panes that speak app-runtime truth, platen owns how any of them dock and
> tile.

Concretely: gloss + roster stay `crates/domain/*`; **trail** earns its domain crate
on the P8 pattern; alembic + steward vocabulary is session-runtime's (persistence
truth) with views app-side; apparatus splits its natures (settings vocabulary =
`domain/apparatus` in mere, the diagnostics feed is HostObservability = app runtime,
so the live pane is merecat's); **inspector is merecat's outright**; **comms**
consumes the Murm and Moot domain services.

## Supply reachable today

The most useful fact for planning this rung, and it appears nowhere in the ladder.
In merecat's `Cargo.lock` **today**, transitively through `mere` and
`session-runtime`, all of these resolve and compile: `frisket`, `platen`, `forme`,
`gloss`, `roster`, `trail`, `apparatus`, `uxtree`, `pelt-core`, and `verso-tile`
(a hard dep of `fetch`; `fetch::cookies::session_cookies_for` already returns
`verso_tile::api::Cookie`). They are in the build graph and merely not nameable.
Naming one costs a dep line, the precedent merecat already uses twice (`fetch` and
`session-runtime` as direct git deps on mere.git).

Two facade gaps: `frisket` and `uxtree` are not re-exported by `mere`, so each needs a
facade export or a direct dep.

Not in the build graph: `cambium`, `sprigging`, `pelt-desktop` (all in
`[[patch.unused]]`), and `scrying-engine`.

So rung 5's **model half needs no repo move and no facade change to start**. Only the
renderer half is blocked, and it is blocked outside merecat.

## Slices

Ordered. Each is independently landable. The toolkit decision comes last on purpose.

### A. The surface plan — LANDED 2026-07-14 (3f043d9)

The seam exists: `surface.rs` holds `Rect`, `Surface { id, kind, rect }`,
`SurfaceId` (content folded from the node uuid so a node keeps its id across
frames), `FocusTarget`, and the pure `plan` / `hit_test` / `focus_for_press`.
`Shell::render` builds the plan, rasterizes each surface at its rect size keyed
by its surface id, and composes each at its rect; `capture_composed` composes the
same list. `App` holds `focus`; `observe::Snapshot` reports `surfaces` and
`focus`; the scenario grammar has `assert surface` / `assert focus`. The rung-4
occlusion bug is fixed (content is inset, canvas visible beside it). Receipts: 29
unit tests, and `scenarios/rung5_surfaces.scn` headed with RESULT ok (surfaces=2
at rest, 3 once content is live; capture measured at [410,1023], the full pane).
`hit_test` / `focus_for_press` are built and tested but not yet wired to pointer
events (slice B). Original scope below.

Follow-up (same day): the first headed capture showed content painting only a
third of its rect with canvas bleeding through, and the commit wrongly blamed a
transparent clear on the vello path. It was a placement bug: `ExternalTexturePlacement`'s
`dest_rect` is `[x0, y0, x1, y1]` corners (the compositor shader reads
`mix(dest.xy, dest.zw, local)`), and `Rect::dest` emitted `[x, y, w, h]`. Every
prior placement was the full window at the origin, where the two conventions
coincide, so slice A was the first non-origin placement to hit it. Fixed in
`Rect::dest`, pinned by a regression test, re-verified headed. The clear was never
the problem.

Build the seam the ladder says rung 3 already built. Replace the three hardcoded
locals and the one shared `full` placement in `Shell::render` with an ordered
`Vec<Surface { id, kind, rect, z }>` in App truth, a composite loop that places each
surface at its own `ExternalTexturePlacement`, an explicit `FocusTarget` on App, and
a top-down rect hit-test that transforms window coordinates to surface-local before
dispatch. Put the rect math, hit-test, and focus resolution in a new `surface.rs`:
they are pure functions and test without a GPU. Generalize `capture_composed`
(currently three positional layer params) to a slice of (view, placement) in the same
commit, or the scenario receipts stop covering panes. Grow `observe::Snapshot` with
`surfaces` and `focus`, and the scenario grammar with `assert surface` and
`assert focus`.

Take the free win while the ids are being minted: merecat calls the unkeyed
`SurfaceHost::rasterize` three times per frame, and genet-winit-host's own doc says
an unkeyed multi-surface host rebuilds every tile on every call.
`host.core().rasterize_for(id, ..)` is reachable today and `SurfaceId` is exactly the
u64 key it wants.

- **Supply**: nothing new. netrender's `ExternalTexturePlacement` already carries
  `dest_rect`, `uv`, and `opacity`; merecat passes `[0,0,w,h]` to everything.
- **Gate**: none. Buildable today.
- **Done**: a scenario opens a node, toggles live content, and the page renders inside
  a rect that is not the whole window, with the canvas still visible beside it.
  `observe::snapshot` reports the surface list and the focus target, and a scenario
  asserts both. The surface module's rect, hit-test, and focus functions have unit
  tests that run headless.
- **Risk**: this is the seam the ladder booked as already-existing, so it is the slice
  most likely to be skipped. Skip it and every later slice hand-writes its own routing,
  which is the imperative crossroads doctrine 2 exists to prevent. Note `shell.rs` is
  696 LOC with zero tests and already holds render, run_effects, act, all input routing,
  and capture. Rung 5 adds rect math, hit-testing, and focus dispatch to exactly that
  file. Split before adding (the 600-LOC ceiling applies).

### B. Content gets input, and stops occluding

Rung-4 debt, and it must clear before panes.

Route pointer, wheel, and keys to the focused surface. Live content sessions receive
nothing today: merecat calls only `pump`, `frame`, and `settled`, while
`inker::DocumentSession` already offers `click_at`, `scroll_by`, `scroll_at`,
`scroll_for_key`, `scroll_to`, and `links`. Fold `SessionClick::Navigate(url)` back
into `Action::OpenAddress` so a link click inside a page grows the graph.

Fix two rung-4 bugs in the same slice:

1. The content layer rasterizes with `ColorLoad::Clear(wgpu::Color::WHITE)` and
   composites full-window, so a live page hides the whole canvas. Invisible at rung 4
   (one page, whole window); a visible bug the moment a pane is smaller than the window.
2. `shell.rs:213-217` frames content for `focused_member()` **only**.
   `ContentStates::live_nodes()` is dead outside its own test. A second live node's
   session is pumped by nothing and framed by nothing, so `Live` is a lie for every
   non-focused node today.

- **Supply**: inker's `DocumentSession` (landed, complete). `SessionLink.rect` is
  viewport-space, so hit-testing content at any placement other than full-window
  structurally requires slice A's per-surface rect. The rect is the precondition.
- **Gate**: slice A.
- **Done**: a scenario clicks a link inside a live page and a new node appears in the
  graph. The page scrolls under the wheel when the pointer is over it; the canvas pans
  when it is not. A second node's content stays live while unfocused. The deletion-matrix
  row "Focus and switch between the graph canvas and documents" passes.

### C. The pane tree: adopt `frisket`

Take `frisket` as a direct git dep on mere.git. It is already in merecat's `Cargo.lock`
transitively through session-runtime; it is merely not nameable, which is why merecat
can already call `session_runtime::frisket_store::{save,load}_frame_layout` but
cannot name the `FrisketLayout` they take.

Hold `FrisketLayout` in App truth, lower summon/close/divider/maximize to Actions, and
persist through frisket_store to `<session_dir>/frame.json`. Drop the three dead
`PaneContent` variants (`System`, `Tile(LeafNodeRef)`, `Custom(String)`), which no host
ever constructs.

Two caveats that are load-bearing:

- `Custom` is used inside frame's own layout ops as a transient sentinel, and there are
  **two** of them: `__placeholder__` (layout.rs:87, 209, 217) and `__dedup_placeholder__`
  (layout.rs:403, in `dedupe_graph_panes`). The replacement needs a placeholder
  affordance or restructured split ops.
- `PaneContent` variant names are the on-disk `frame.json` serde tags, with no rename
  attrs. `PaneContent::Orrery` therefore stays `Orrery` on disk even though the crate is
  now `canvas`. Renaming it is a format migration, already parked as "a separate
  vocabulary decision" in the boundary-pass plan. Do not sweep it with the names.

Whatever replaces `PaneContent` must still answer `follows_active_graph()` (the
multi-graph re-sourcing policy behind `retag_graph_bound`), a stable serde tag, and
`tag()` (tracing plus accessible names).

`frisket` is MPL-2.0 and mere-original (not Servo-derived), so relicensing to MIT/Apache
on the way over is legitimate, but it is a named step, not a silent one.

- **Supply**: `frisket` (1,249 LOC, 14 tests, host-neutral, geometry-free).
- **Gate**: slice A. The surface plan is what turns frame's ratio tree into rects.
- **Done**: open a pane, drag its divider, maximize it, close it. Restart and the
  arrangement comes back off `frame.json`. This bar is lower than it sounds and should
  be stated so: every meerkat summon is a fixed Right-split off the graph pane at a
  hardcoded ratio, and `frisket::reparent_leaf` is dead in the host (called only by
  frame's own tests). Pane drag-rearrangement is **not** a bar merecat must clear at the
  frisket tier.
- **Risk**: rewriting `frisket` instead of adopting it invalidates the `frame.json` format
  session-runtime already ships (`PaneNode::Leaf` even carries `#[serde(default)] graph_id`
  to keep pre-graph_id layouts loadable, so the format already has a migration history).

### D. The first non-canvas panes, on the DOM path rung 3 proved

Render a pane's content as a `ScriptedDom` subtree laid out by genet-layout into a paint
list and composited at its surface rect. That is exactly the path `ui::chrome_scene`
already runs.

Order matters, and it is set by dependency weight:

1. **trail** first: zero dependencies (its Cargo.toml has no `[dependencies]` table at
   all), 148 LOC, four pure functions around `build_trail_items(&TrailInput) -> Vec<TrailItem>`.
   Its own header says it was extracted on the P8 pattern (explicit inputs in, neutral
   items out, host maps items to rows) as the model the other extractions should follow.
2. **roster** next: 1,290 LOC, deps forme and kernel only, the biggest single payoff in
   the supply.
3. **inspector** after: merecat's outright per the taxonomy, no crate by design. The
   donor is `meerkat/src/inspector.rs` (488 LOC) over
   `inker::{Block, DocumentDiagnostic, DocumentTrustState, EngineDocument}` and
   `session_runtime::browser_node_state::BrowserNodeState`. It is fetch/content
   introspection, so it is app-runtime truth and belongs here.
4. **gloss** last of the four. It is **not** view-free the way roster and trail are: it
   returns a `netrender::Scene` from `minimap_backdrop_scene` and takes a
   `register_theme::chrome::ChromeTheme`, and its lock deps drag the entire `canvas`
   crate plus `register-theme` into the graph. It constrains the render seam, so it goes
   after the seam is proven.

- **Supply**: `trail`, `roster`, `gloss` (all in the lock, all facade-re-exported);
  inspector is ported, not fetched.
- **Gate**: slices A and C.
- **Done**: the Trail pane opens beside the canvas, lists real recent and history rows off
  graph truth, and a click on a Recover row lowers an Action through the same spine as a
  keypress. A scenario drives it and asserts a row's text.
- **Risk**: `ui.rs` rebuilds its DOM wholesale per state change; its own module doc says
  so. Fine for a seven-entry palette (`palette_actions()` returns seven), and not fine for
  a roster with hundreds of rows and clickable facet cards. That pressure is the trigger
  for the toolkit question below, not a reason to defer this slice.

### E. The workbench pane: platen inside a frisket leaf

`PaneContent::Workbench` is one frisket leaf whose content is a platen `Workbench`. This is
where platen earns its place and where the renderer question bites. Two options, and this
is the real decision, not a detail:

- **(a) Take pelt-desktop's `tile-surface` seam.** Inherit meerkat's proven tile surface,
  tab chrome, divider and drag gestures, drop resolution, a11y tree, and TileEvent loop
  (`TileFrame { frame_scene, tiles: Vec<TileLayer { tile, rect, scene }> }`, plus
  `TileShell`).
- **(b) Walk platen's `WorkbenchPlan` fraction tree into rects host-side** and render it on
  the ScriptedDom + genet-layout path merecat already runs, keeping merecat's dep graph free
  of pelt and cambium.

Preserve platen's persistence discipline verbatim either way. The canonical
`(Arrangement, TreeGeometry)` pair with `canonical_roundtrips()` debug-asserted and a
600-seed fuzz test is the strongest thing in the whole supply.

- **Supply**: `platen` (already facade-exported, already in the lock) plus `pelt-core`'s
  tile contract (also in the lock). Note platen is view-framework-free but **not** dep-free:
  it deps `document-canvas`, `inker`, `pelt-core`, `forme`, serde. Option (a) additionally
  needs `pelt-desktop`, `cambium`, and `sprigging`, none of which are in merecat's build
  graph.
- **Gate**: option (b) gates on nothing beyond slices A and C. Option (a) gates on the
  cambium/genet duplication being resolved (see Watch items).
- **Done**: two nodes tile side by side inside the workbench pane. Drag one onto the other's
  tab bar and they stack. Drag a divider and the split reweights. Restart and the tiling
  comes back through platen's persistence pair.
- **Risk**: platen supplies the model and the mutators, nothing else. The host glue that
  drives it is roughly 1,267 LOC in meerkat (`input/workbench.rs`, `frame_ops/panes.rs`,
  `frame_a11y_panes.rs`). The glue, not the model, is the work, and it is the same work under
  either option. Option (a) pays it once by inheriting; option (b) pays it by re-deriving
  roughly 1,500 LOC of tab, divider, and drop machinery.

### F. A11y over panes, and the honest capability report

The deletion matrix requires a coherent snapshot and accessibility tree. Merecat has no
accesskit dep, no uxtree dep, and calls no projector.

The projectors exist and are unused:

- `frisket::project_frisket` / `project_frisket_with` (frame/src/projection.rs): the pane-tree
  projector. Arrives free with slice C.
- `mere::workbench::project_workbench(&Workbench) -> UxTree`: platen's tiling.
- `uxtree::project_document(&EngineDocument) -> UxTree` and
  `gloss::project_outline(&EngineDocument) -> UxTree`: documents.
- `genet_layout::build_subtree(dom, fragments, root, id_of, skip)`: the stitching primitive,
  whose own doc says it is for a host that stitches several subtrees (chrome, content panes,
  host root) into one tree, with id-salting and skip-pruning. Use
  `build_subtree_with_leaves` if sprigging leaves ever enter.
- `uxtree::stitch`: mere's stitcher, in merecat's lock but not facade-re-exported.

- **Supply**: all of the above. Only `uxtree` needs a facade export or a direct dep.
- **Gate**: slice D. There must be panes to stitch.
- **Done**: the window's AccessKit tree carries chrome plus each pane's subtree under one
  root with disjoint id ranges. A document pane announces as a region and reports its
  capability honestly rather than implying coverage it does not have.
- **Risk**: the missing piece is precise and small, and it is **not** merecat's to fix. A live
  `DocumentSession` has no accessor to its `EngineDocument`, so a live document cannot be
  projected: `LoadedDocument`'s fields are private (pub methods stop at
  `inspect() -> ContentReport`), and merecat's one registered engine is genet's
  `StaticSessionEngine`, whose session type `StaticDocumentSession` is **private** (only
  `ScriptedDocumentSession` and `SmolwebDocumentSession` are `pub`). Meerkat solves this by
  downcasting through `as_any` to `HostScriptedDocument`, its **own** type. Merecat cannot:
  for the one lane it ships, the downcast option does not exist. This is a genet ask (put a
  projection accessor on `DocumentSession`), and it is the only path. Until it lands,
  `A11yCapability::Partial` is the honest answer and the pane should say so, per the
  no-placebo rule.

## Not carried

- **`steward` and `alembic` as domain crates.** Neither exists anywhere in the tree.
  `steward` is `meerkat/src/steward.rs` (307 LOC) reading meerkat's `Constellation` actor pool
  and SyncIndicator: live host state with no mere-side vocabulary, and its charter is half a
  comms surface, so part of it is rung-8 material. `alembic` is roughly 155 LOC inline in
  meerkat's `pane_data.rs`; its engine half is landed and pure in session-runtime (athanor's
  propose/apply passes, memory_levels, graph_engram), which merecat already git-deps. Per the
  taxonomy both are session-runtime vocabulary with views app-side, so they are pane views to
  build, not crates to fetch.
- **`apparatus` as working supply.** The crate exists and is facade-re-exported, so it scans as
  supply. It emits four **empty** AccessKit groups, its only pub fn is `project_skeleton()`, it
  has zero use sites, and meerkat quietly runs its own 465-LOC `apparatus.rs` instead. Budget it
  as a stub or the Apparatus pane ships blank.
- **Notes and Settings as panes.** Notes is the document-family Block-to-view renderer for cards
  and tiles (rung 4's content lane) plus a chrome overlay, `knot_editor_pane`. Settings are
  per-`settings://`-node workbench **tiles** positioned at tile-body rects. Neither is a
  `PaneContent`. Do not schedule them as rung-5 panes.
- **Pane drag-rearrangement at the frisket tier** (slice C, above).
- **Tear-out.** It is rung 7, not rung 5: it is multi-window by definition. Its supply is
  `2026-07-08_portable_tiles_plan.md` (cross-window DOM identity), whose own header says it is
  blocked on step 3 of `2026-07-08_forest_dom_plan.md`, names meerkat as its consumer, and rides
  genet's `PortableKeyed` / `Moved` / nursery.
- **meerkat's `sync.rs` protocol assembly.** mere's peer-runtime plan has an explicit stop rule
  against leaving operation verification or insertion callbacks in merecat. That wiring is deleted
  by its Phase F and must not be ported.
- **`ContentPane`.** A `pub(crate)`, two-variant, non-serde nav-focus discriminator
  (Orrery | Workbench) meaning "which content pane does the omnibar act on". It is not a
  pane-identity type and dropping it costs nothing.

## Open questions

These are Mark's, not the implementer's.

1. ~~**Where does the pane model live?**~~ **DECIDED and LANDED 2026-07-14 (with Mark).** The crate
   was misnamed and mis-cut, which is what made the question hard. `frame` was two crates in one
   coat: mere's workspace **identity vocabulary** (`GraphId`, `SessionId`), which `crawl` and
   `session-runtime` depend on and which has nothing to do with panes, fused with the **pane
   model**. Split at that seam:

   - **`incipit`** (new, `crates/incipit`, serde + uuid only): `GraphId` + `SessionId`. Stays in
     mere. It is what lets `crawl` name a graph without depending on `kernel`.
   - **`frisket`** (renamed from `frame`, `crates/shell/frisket`): the pane model. On a hand press
     the frisket is the frame whose cut-out apertures decide what prints where.

   Destination: **merecat**, because `PaneContent` names panes merecat owns outright (Inspector,
   Comms), and a library crate enumerating the app's panes is an inversion. It **cannot move yet**:
   meerkat still depends on it. So it stays a mere crate and merecat git-deps it, exactly as merecat
   already does with `fetch` and `session-runtime`. It relocates when meerkat is deleted, and
   `session_runtime::frisket_store` goes with it. That is the session-runtime split the
   boundary-pass plan already parked as a follow-on, so this inherits an existing gate rather than
   inventing one.

   Two names deliberately left on disk, to be changed together in one format migration rather than
   as two silent breaks: the sidecar is still `frame.json`, and `PaneContent::Orrery` is still
   `Orrery` (a serde variant name) even though the crate is now `canvas`.

   Receipts: frisket 14 tests green, incipit 3, session-runtime 188, meerkat compiles.
2. **Workbench-pane renderer: pelt-desktop's `TileSurface`, or hand-rendered on the ScriptedDom
   path?** Slice E, options (a) and (b).
3. **Does merecat ever adopt a view framework** (cambium + sprigging + register-theme), or do panes
   stay on hand-built DOM? Rung 3 shipped without one and shipped well. Roster's hundreds of rows
   is where the answer starts to matter. Right now this decision is being made by omission, which
   is the worst way to make it.
4. **Does `workbench` (crates/platen/domain/workbench, 164 LOC) move with platen?** It hard-deps
   platen, so when platen moves to merecat it moves or breaks. No doc says. If it does **not** move,
   a library crate ends up depending on the app's crate, which is an inversion. It would otherwise
   be discovered at the compile error.
5. **Which settings are app truth versus persisted?** Not "where do they live": session-runtime
   already ships `settings_store` (`PersistedSettings`, `ShellbarEdge`, `ScriptPermissionPrefs`,
   `save_settings`/`load_settings`, `<session_dir>/settings.json`, atomic write, additive-field-safe),
   and merecat direct-deps it. The real gap is ownership: three live mere plans (peer-runtime,
   deletion/retention, native-drop) assign retention, transport-preference, and export-profile
   settings to merecat by name, and the ladder has no settings module and no settings rung.
6. **Does Steward ship content-ops-only at rung 5** and grow its sync rows later, or does merecat take
   a rung-8 dependency early? Per the no-placebo rule the readout must be genuine or absent; a fake
   sync row is not an option. The cheap honest answer is a status port that returns empty until comms
   lands, the same shape the content port already uses.
7. **Should file ingress (OS file drop plus a picker) get a rung?** Merecat handles no winit
   `DroppedFile` or `HoveredFile`. It is independent of comms, it serves the ordinary "open a local
   file as a node" case, and it is the app-facing seam of the whole native-drop lane
   (`import_plain_drop_file(path, ...)`). The deletion matrix's last row makes the omission a defect
   rather than a wishlist item.

## Watch items

Neither is a dependency; both landed after the ladder was written.

1. **The Cambium duplication is live.** repos/cambium holds the extracted toolkit (packages
   `cambium`, `meristem`, `sprigging`, `cambium-nematic`, `cambium-winit`). Its C5 step (delete the
   copies in genet) has **not** run: `components/xilem-serval`, `components/xilem-core`, and
   `components/chisel` are still genet workspace members. Two live, diverged copies of the toolkit.
   mere's HEAD (`271e79d`, "Point Meerkat UI at Cambium") already points meerkat at cambium through
   its gitignored `.cargo/config.toml`, so mere's committed manifest builds only because of a local
   file. Merecat is the only family member currently insulated from this, because it took no toolkit
   dep. That insulation is an asset; spend it deliberately. Note the flagship component (the
   filterable action list) does **not** exist in cambium: `menu()` is render-only and the host owns
   query, selection, and keyboard. Do not budget it as supply.
2. **`livery`.** `livery` 0.0.2 and `genet-livery` 0.0.2 are genet workspace members at HEAD: a
   clean-room, generated CSS property and cascade engine. `genet-livery` deps `layout-dom-api`,
   `paint_list_api`, and taffy, the same seam merecat's `ui.rs` already drives through genet-layout.
   Genet's own audit names Cambium as first consumer and says it "does not claim to be Merecat's
   production theme". Watch, do not plan against it; the swap would be confined to `ui.rs`'s layout
   and paint call.
