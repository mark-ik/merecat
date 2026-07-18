# Merecat architecture: obviating meerkat

2026-07-10, refreshed 2026-07-14. The founding doc set the role swap (mere =
library, merecat = the reference host); the boundary pass sharpened what mere
keeps. This plan is the other half: what merecat IS, structurally, and the
ladder by which meerkat becomes unnecessary. Companion to
[2026-07-08_merecat_founding.md](./2026-07-08_merecat_founding.md),
[2026-07-14_merecat_rung5_panes_plan.md](./2026-07-14_merecat_rung5_panes_plan.md),
and mere's 2026-07-09 boundary pass plan (its decisions are assumed here, as
amended: canvas stays mere; platen is the node-tiling home inside the workbench
pane and arrives with the port; the pane model itself is mere's `frisket` crate,
whose destination is undecided; the verso family is genet's `verso-tile`).

Platen is **not** the pane home, and the 2026-07-10 text that said so is the
root of rung 5's misframing. Every platen `Workbench` mutator takes a
`forme::GraphMemberId` (the kernel `Node` UUID), so a platen tile can only ever be
a graph node. The pane model with kinds, ids, a split tree, and an on-disk format
is `mere/crates/shell/frisket`. See the rung-5 plan.

## Doctrine

1. **Composition, not migration.** Merecat is assembled from the promoted
   libraries. Meerkat is the donor and the behavioral reference; its modules
   are read for technique and product decisions, never copied wholesale. (The
   same posture the family holds toward graphshell.)
2. **One vocabulary for everything that acts.** Meerkat's largest modules are
   imperative crossroads (`command_drain` 1,196 lines, `shell_eval` 884)
   because commands, input, scripts, scenarios, and UI each grew their own
   mutation paths. Merecat has one: everything lowers to a typed `Action`;
   effects leave through ports; services answer with typed updates. Settings,
   automation (the native-automation plan's WebDriver adapters), scenarios,
   scripting, and remote control all speak Action, so no lane needs a second
   execution model.
3. **Modules first, crates when a boundary proves itself.** The app is one
   crate today. The module map below is the crate map of the future; a module
   is promoted to a crate when a second consumer or a build-condition needs
   the boundary (the same gate the family applies everywhere). No speculative
   workspace sprawl.
4. **Engines arrive through the registry.** The content lane waits for the
   inker adoption (another agent's migration): merecat registers engines and
   composites frames; it never hand-dispatches lanes the way meerkat's
   content actor does. Hand-wiring is the duplication the registry exists to
   prevent.

   *Amended 2026-07-14: the registry gives you spawn, not observation.* Dispatch,
   input, and the link hit-table are on the `DocumentSession` trait, so those lanes stay
   hand-dispatch-free. The document INTERIOR is not: a11y projection and DOM
   introspection reach the concrete type through `as_any`, which the trait's own doc
   comment says. Meerkat pays this at four downcast sites, but it downcasts to
   `HostScriptedDocument`, its **own** type. Merecat cannot follow: its one registered
   engine is genet's `StaticSessionEngine`, whose session type `StaticDocumentSession` is
   private, and `LoadedDocument`'s fields are private with no DOM accessor. For the lane
   merecat actually ships, the downcast option does not exist. So this is a genet ask (a
   projection accessor on `DocumentSession`), and it is the only path, not a fallback.

## The spine

```text
platform event (winit / a11y / automation / scenario)
  -> Action (typed; the one vocabulary)
  -> update(state, action) -> Effect list
  -> ports (fetch actor, physics actor, persistence, engines, comms, ...)
  -> Update (typed service answers, drained on wake)
  -> update(state, update) -> Effect list
  -> projection (canvas Scene now; chrome DOM + pane tiles later)
```

Rules that make this real rather than aspirational:

- `update` never blocks and never touches a platform API. Anything slow or
  platform-shaped is an `Effect` handled by a port.
- Ports are the only owners of actors and stores. State holds data, not
  handles (the shell owns the handles and runs the effects).
- The vocabulary stays port-agnostic: `Update` carries app-owned message
  types; adapters (browse, later content/comms) convert each service's
  concrete types at the port boundary.
- **The gesture law**: ephemeral interaction may bypass Action (continuous
  pointer/wheel maps onto the canvas's semantic input methods, which are
  already typed vocabulary); durable or externally observable semantic
  change may not. A gesture that ends in one (placing a node, committing a
  viewport, changing selection/focus) surfaces a semantic event at gesture
  end. Durable positions already have the family answer (the
  cartography-geometry sidecar, persisted at session save); the events are
  for observability.

### Recorded for later, with triggers (2026-07-10 review round)

A second-agent critique sharpened the plan; these are adopted as decisions
whose implementation waits for a named trigger, so nothing designs against
them in the meantime:

- **Observation is the vocabulary's other half.** A snapshot/event pair
  (application snapshot: windows, surfaces, focus, selection, available
  actions, content state; plus an event stream) serves AccessKit,
  automation, diagnostics, and scenarios from ONE surface. Trigger: rung 3's
  chrome lands born-observable; the pair arrives with its first
  scenario/automation consumer (the native-automation plan's convergence
  point). A11y-as-semantic-projection is therefore NOT long-tail material.

  *Landed 2026-07-12 (f3e0a5c), but short of this charter, so do not read it as
  discharged.* `observe::Snapshot` carries focused node, omnibar view, content lifecycle,
  node count, and graph visibility. It carries no windows, no surfaces, no focus target,
  and no available actions. Today no scenario can assert which surface is showing or
  focused, which is exactly the receipt rung 5 needs. `available actions` is nearly free
  (`action::palette_actions()` is already the registry the `>` lane commits through);
  `surfaces` and `focus` arrive with rung-5 slice A; `windows` honestly waits for rung 7.
- **Action envelopes + ingress authorization.** Identity, source, target,
  and outcome on actions; authorization at ingress (where personae/kith
  capability grants plug in). Trigger: the first non-local action source
  (automation/remote); targets with multi-window (rung 7).
- **Multi-window shape.** One `Canvas` per GRAPH (not per window), pooled in
  app truth; per-window/pane cameras via the canvas's existing
  `viewport()`/`set_viewport` install seam (the camera is already off the
  shared authority — meerkat proves this in production). Window records and
  semantic focus join `App` at rung 7; no `Arc<Mutex>`, no
  canvas-as-projection rewrite.
- **Async boot/shutdown.** Today's sync boot IO (before an event loop
  exists) and sync close-save are honest; when persistence goes async
  (eidetic stores), boot becomes `Effect::LoadSession -> Update::SessionLoaded`
  and close becomes `RequestClose -> PersistSession -> SessionSaved -> exit`.
- **Correlation over URLs.** Enrichment keys by stable node id (URLs are
  properties; several nodes can share one, and `get_node_by_url` answers
  first-match). Armillary now supplies host-neutral `RequestId`/`RequestIds`
  and `Correlated<T>`; the content lane should use those for command/update
  pairing and retain app-owned staleness semantics for late results against
  superseded nodes.

## Module map (the future crate map)

| Module | Owns | Today | Crate when |
| --- | --- | --- | --- |
| `action` | `Action`, `Effect`, `Update` enums (port-agnostic) | landed | headless automation needs `action + app` without a shell |
| `app` | `App` state + the two `update` fns | landed | never alone; travels with `action` |
| `browse` | address opening, fetch, redirects, metadata enrichment, favicon discovery (the fetch adapter) | landed (was `web`) | another app consumes the port |
| `content` | per-node document lifecycle (`NodeContent` / `ContentStates`) | landed (rung 4, d1e6234; 109 LOC) | feature isolation changes the dep graph |
| `session` | persistence port: graph.json now; browser_nodes.json, view intent, multi-session later | landed | multi-session lands |
| `settings` | engine/viewer settings as app truth; retention, transport-preference, export-profile (assigned to merecat by three mere plans) | absent | it exists at all |
| `shell` | winit + SurfaceHost + layered present + input routing + effect runner | landed (696 LOC, 0 tests; over the 600 ceiling) | a second host (wasm) appears, or desktop/web shells share the core |
| `surface` | surface list, rect math, hit-test, focus resolution | absent | rung 5 slice A births it |
| `pane` | `FrisketLayout` in app truth, summon/close/divider Actions | absent | rung 5 slice C, if `frisket` is adopted |
| `observe` | `snapshot(app)` + the `AppEvent` stream | landed (175 LOC) | AccessKit or automation consume it out-of-process |
| `script` | capability-scoped Piccolo control scripts that read an app snapshot and emit `Action`s | landed (feature-gated first slice; host API only) | an explicit command/automation consumer or a second host appears |
| `scenario` | the self-drive grammar + GPU self-capture | landed (517 LOC) | test-only builds change the dep graph |
| `ui` | chrome DOM (omnibar, caption chip); pane tiles later | landed (rung 3; 534 LOC, 6 tests; hand-built `ScriptedDom` laid out by genet-layout, emitted as a paint list, composited as the chrome layer; no view framework) | a second chrome consumer appears, or a toolkit adoption changes the dep graph |

The `content` row's 2026-07-10 charter listed five things; the module owns one.
Engine registration is `shell.rs:85-86`, frame composition is `shell.rs:212-223`,
and session handles are `shell.rs:60`, all by design (ports own the non-Send
handles, which content.rs's own header says). The verso-tile flip and content input
routing do not exist at all; they are rung-5 work, because they need the surface
plan. Registration and frames stay in `shell`.

Crate promotion is gated on consumers and build shape, never on module size:
headless automation wanting `action + app`, a second shell, another app
consuming a port, or feature isolation that changes the dependency graph.
The likely eventual shape is `merecat-core` / `merecat-desktop` /
`merecat-web`; until a gate fires, modules are correct.

## Portable app-control scripts

Piccolo belongs in Merecat for user-authored commands, workflows, and small
pieces of portable glue that need Rust-native host bindings. It is not the
browser-page JavaScript lane. Genet's Vano/Nova backend remains the primary
64-bit JavaScript engine, with Boa as the pure-Rust and wasm/conformance lane;
Piccolo is the stackless-Lua mod/control option.

The first Merecat slice is deliberately narrow and feature-gated behind
`piccolo`. A host supplies a snapshot and the script can call:

- `mere.snapshot()` — read a JSON app summary;
- `mere.open(address)` — emit `Action::OpenAddress`;
- `mere.dispatch(name)` — emit an existing named `Action`;
- `mere.summon(kind)` — emit `Action::SummonPane`.

The host applies those actions through the existing `App::update` spine. The
script receives no `App` reference and has no filesystem, network, process, DOM,
or page-JavaScript binding. Read, dispatch, navigation, and pane capabilities
are separate, and execution has a step budget. This is a host seam, not yet a
user-facing omnibar or command-registry binding.

The next surfaces are separate decisions: a richer read-only graph query
snapshot, graph mutations lowered to new Actions, pane/chrome extension
registration, and an explicit user-configurable workflow entry point. They
should not be smuggled in as arbitrary Lua access to app state.

## Supply reachable today

The most useful fact for planning the remaining rungs, and it was absent from the
2026-07-10 draft. In merecat's `Cargo.lock` **today**, transitively through `mere`
and `session-runtime`, all of these resolve and compile: `frisket` (the pane model),
`platen`, `forme`, `gloss`, `roster`, `trail`, `apparatus`, `uxtree` (the a11y
stitcher), `pelt-core` (the tile contract), and `verso-tile` (a hard dep of `fetch`).
They are already in the build graph and merely not nameable; naming one costs a dep
line, the precedent merecat already uses twice (`fetch` and `session-runtime` as
direct git deps on mere.git).

Two facade gaps: `frisket` and `uxtree` are not re-exported by `mere`.

Not in the build graph: `cambium`, `sprigging`, and `pelt-desktop` (all sitting in
`[[patch.unused]]`), and `scrying-engine`.

So the model half of rung 5 needs no repo move and no facade change to start. Only
the renderer half is blocked, and it is blocked outside merecat.

Rung 3 births the **layered present** in its crudest honest form, and the
2026-07-10 text overstated it. What exists: `Shell::render` rasterizes a fixed set
of scenes (canvas always, the focused node's content session when live, the chrome
layer when it has content) and composites them in hardcoded order, every one of them
full-window (`let full = ExternalTexturePlacement::new([0.0, 0.0, w, h])`, reused for
all three). There is no surface list, no ids, no z-order, and no rects. Input routing
is one boolean, `self.app.omnibar.open`, branched at three sites; continuous gestures
(CursorMoved, MouseWheel) do not consult even that and go unconditionally to the
canvas. A live content session receives no input at all.

The ordered surface list (ids, z-order, per-surface rects, per-surface input routing)
is therefore **not** an inherited asset. It is rung 5's first slice, and rung 4 is not
finished until it exists, because the content surface is already on screen and already
broken by its absence: it composites opaque (`ColorLoad::Clear(WHITE)`) at full window,
so a focused node with live content completely occludes the graph canvas.

`Shell::render` must never become the next imperative crossroads. It is 696 LOC with
zero tests and already holds render, run_effects, act, all input routing, and capture.
Rung 5 adds rect math, hit-testing, and focus dispatch to exactly that file. Split
before adding.

mere::canvas is hosted, not wrapped: the shell maps raw input onto the
canvas's semantic methods directly (they are already the right vocabulary),
and canvas-affecting Actions call them from `update`'s effect handler. A
merecat-side canvas facade would be indirection with no second consumer.

## The obviation ladder

Meerkat dies when merecat is the daily driver. Each rung names the meerkat
capability, its source of supply, and its gate. Rungs are ordered by
daily-driver value, not by meerkat's module sizes.

1. **Graph canvas + persistence + enrichment**. Supplied by mere::canvas, fetch,
   session-runtime. LANDED (slices 1-4, 2026-07-09): open address, fetch,
   title/mime/favicon stamps, session restore.
2. **The Action spine**. This plan's first slice: restructure the bin onto
   action/app/web/session/shell modules, behavior-preserving. Gate: none.
3. **Omnibar + navigation chrome**. COMPLETE 2026-07-11, but not from the supply
   this rung named. Merecat has zero xilem dependency: the chrome is a hand-built
   `genet_scripted_dom::ScriptedDom` over a `&'static str` sheet, laid out by
   `genet_layout::IncrementalLayout`, emitted as a paint list and composited by
   `paint_list_render::composite_paint_layers`. mere's `chrome` crate really does hold
   `NavTarget`, `History`, and `suggestions(query, history)`, but the mere facade does
   not re-export `chrome`, so merecat cannot reach it. Merecat rolled its own
   `ui::Suggestion`, `normalize_address`, and `recompute_suggestions` matching GRAPH
   truth. Record `suggest` as **superseded by design**: graph search is the right
   product for merecat, not history-made-spatial. `nav` is **still owed**, not
   superseded: the `Action` enum has no Back, Forward, or Reload variant, while the
   deletion matrix requires all three. That owed work moves off this rung (below).
4. **Live content on nodes**. LANDED for the static lane 2026-07-11 (d1e6234). Gate
   (the mere->genet agent's migration, adoption done-conditions 1-6) FIRED. The rung
   delivered about a third of its charter, so name the rest here rather than let a
   fired gate read as a finished row. NOT landed: exactly one engine is registered
   (`StaticSessionEngine` under `genet.web`), and `genet-documents` is pulled with
   `features = ["netfetch"]` only, so scripted and smolweb are not compiled in (they
   join by a feature flip plus two register calls, not new dispatch); the compat lane
   ("scry tiles + verso-tile flip") does not exist, since there is no `scrying-engine`
   dep (`verso-tile` IS in the graph, through `fetch`); the content frame is a
   full-window opaque layer, not a node-sized tile; no input reaches a session; and only
   the focused node is pumped and framed, so `Live` is a lie for every other node. The
   compat lane and content input are rung-5 work, because they need the surface plan.
5. **Panes**. Scoped in
   [2026-07-14_merecat_rung5_panes_plan.md](./2026-07-14_merecat_rung5_panes_plan.md).
   Supply: `frisket` (the pane model: PaneId, PaneContent, a PaneNode split tree,
   FrisketLayout, frame.json through session-runtime) plus `platen` (the node-tiling model
   INSIDE one `PaneContent::Workbench` leaf) plus the domain crates that actually exist:
   `gloss`, `roster`, `trail`, and `apparatus` (a skeleton that supplies nothing yet).
   Strike "alembic/steward" from the domain-crate list: neither is a crate. Per the
   2026-07-10 pane-content decision they are session-runtime vocabulary with views
   app-side. Gate, restated honestly: BUILD the surface composition and focus routing
   seam (it does not exist), then adopt `frisket`. Platen supplies no pane model and no
   renderer. The renderer is the one genuinely blocked piece and this rung never named
   it: platen emits fractions, never rects; `platen-view` does not exist (deleted
   2026-06-15, superseded by pelt); the only working platen renderer is pelt-desktop's
   `TileSurface`, which drags `cambium` and `sprigging`. Name the choice: take
   pelt-desktop, or render `WorkbenchPlan` on the ScriptedDom path merecat already runs.
6. **Multi-session + browser-state sidecar**. The session-runtime manifests,
   sessions/<id>/ layout, browser_nodes.json. Gate: rung 4 (per-node browser
   state is worth persisting once nodes hold live content).
7. **Multi-window**. The one-state-N-windows doctrine over the Action spine
   (a window is a projection, so the spine already has the right shape).
   Gate: rung 3.
8. **The long tail**. Comms and community services (Murm direct exchange +
   Moot over `murm-replication`), intel (embed/infer glue), import/crawl,
   scripting (Piccolo app-control plus Vano/Boa document-host lanes), theming (register-theme/tinct). Each is a
   port + Actions. A11y projection is NOT here: it is rung 5 (see this plan's own
   "recorded for later", which pulled it out of the long tail; the 2026-07-10 text
   contradicted itself by leaving it in this row). Gate, which this rung lacked:
   Murm and Moot are **not** promoted libraries. `murm-replication` lives at
   `mere/crates/murm/replication`, inside mere's workspace; there is no repos/murm and
   no repos/moot; the mere facade re-exports none of comms, murm, moot, or mesh; and
   the only workspace consumer of `comms` is meerkat. Promotion is the peer-runtime
   plan's Phase G, gated on Phases A through F. Until Phase G lands, merecat cannot name
   Murm or Moot in Cargo.toml at any rung. The founding doc says merecat's Cargo.toml
   "should read like the ecosystem map: mere, personae, murm, moot, genet"; two of those
   five cannot be written today.

**Meerkat's deletion condition** (the founding doc's done-condition 2 made
concrete; behavioral receipts, not a subjective trial — 2026-07-10 review
round): every row below passes, at which point daily-driving merecat is
confirmation rather than the specification.

Each row carries its rung. They were unannotated in the 2026-07-10 draft, which is
how a rung-5 receipt came to read as rung-4-completable.

- (r3/r4) Open addresses from the omnibar; navigate live content. **Met** for the
  static lane.
- (r3) Back, forward, reload, and redirects behave. **Met 2026-07-18**: `NavBack` /
  `NavForward` over `chrome::nav::History` (direct-dep'd; Back re-selects, never
  refetches; a new open truncates the forward branch) + `Reload` (refetch + live
  content respawn), on Alt+Left/Right and Ctrl+R and the palette. Receipts: the
  spine unit test + `rung3_nav.scn` headed RESULT ok. Redirects were rung-1 fetch
  behavior.
- (r5) Focus and switch between the graph canvas and documents. **Unmet, and not a
  rung-4 item**: content sessions receive zero input, and the only focus concept in the
  codebase is the graph node's. Needs rung-5 slices A and B.
- (r5) Open, arrange, and restore panes. Split in two, because "arrange" hides two
  tiers:
  - **Frisket tier** (the low bar, clear it early): summon, divider drag, maximize, close,
    restore. Every meerkat summon is a fixed Right-split at a hardcoded ratio, and
    `frisket::reparent_leaf` is dead in the host.
  - **Tile tier** (the real arrange, and what meerkat actually ships): merge onto a tab
    bar, split on an edge, restore the tiling. Tear-out is **rung 7**, not this row: it
    is multi-window by definition, and its supply (`2026-07-08_portable_tiles_plan.md`)
    is itself blocked on step 3 of the forest-DOM plan.
- (r6) Restore graph, browser state, and content state after a restart.
- (r4, not r6) Change engine/viewer settings and see them apply. The module map filed
  settings under `session`, gated on rung 6, so the matrix demanded at rung 4 what the
  map delivered at rung 6. Settings now have their own module-map row.
- (r5) Run the same scenario through keyboard input and through automation Actions (one
  description, two runners). Merecat now has the scenario runner plus a
  feature-gated Piccolo control runner that emits Actions, but Piccolo does not
  yet run the scenario grammar. The grammar also cannot drive panes: no pointer
  verbs, no element verbs, no surface targeting. `settle` is still frame-counting
  (default 20) though the shell already reads `session.settled()`.
- (r5) Produce a coherent application snapshot and accessibility tree. The snapshot
  landed missing four of its six promised members (no windows, surfaces, focus target, or
  available actions). The a11y tree does not exist in merecat at all: no accesskit dep,
  no uxtree dep, no projector call.
- (r4/r6) Recover from a failed fetch, a failed engine start, and an interrupted save.
- (unrunged) Drop a file on the window: it becomes a node, or textures the node under it.
  Merecat handles no winit `DroppedFile` or `HoveredFile`. The last row below makes this
  a defect rather than a wishlist item, and it needs a rung.
- Meerkat holds no capability absent from this matrix.

Then meerkat leaves mere's workspace and the mere facade drops its two
compatibility re-exports, `platen` and `workbench`, in the same pass. The facade
re-exports twelve crates and only those two are scaffolding; the other ten
(apparatus, canvas, forme, gloss, glossary, graphlets, kernel, linked_data, roster,
trail) are the permanent library boundary. `workbench`'s move is an inference from the
code (it hard-deps platen and lives inside `crates/platen/domain/workbench`), not a
recorded decision, and it needs Mark's confirmation: if it does not move, a library
crate ends up depending on the app's crate, which is an inversion.

## What is deliberately NOT carried

- meerkat's `command_drain` / `shell_eval` execution model (replaced by the
  Action spine; the command REGISTRY vocabulary is worth rereading when rung
  3 adds a command palette).
- The bin-module sprawl (85 modules in main.rs). Merecat's bin stays a shell.
- `ContentPane`. It is a `pub(crate)`, two-variant, non-serde nav-focus
  discriminator (Orrery | Workbench) meaning "which content pane does the omnibar act
  on". It is not a pane-identity type, and dropping it costs nothing.
- (Corrected 2026-07-14.) The 2026-07-10 text said pane identity gets designed at rung
  5 "against platen's model". Platen has no pane identity and no pane kinds, so that
  sentence sent the implementer at the wrong crate. Pane identity is designed against
  `frisket`, and mostly inherited from it: `PaneNode::Leaf { pane_id: PaneId(u64),
  content: PaneContent, graph_id: GraphId }`. Whatever replaces `PaneContent` must still
  answer `follows_active_graph()` (the multi-graph re-sourcing policy behind
  `retag_graph_bound`), a stable serde tag, and `tag()` (tracing plus accessible names).
  Drop `System`, `Tile(LeafNodeRef)`, and `Custom(String)` on the way over: no host
  constructs them. Two caveats. `Custom` is used inside frame's own layout ops as a
  transient sentinel, and there are two (`__placeholder__` and `__dedup_placeholder__`),
  so the replacement needs a placeholder affordance. And `PaneContent` variant names are
  the on-disk `frame.json` serde tags: `PaneContent::Orrery` stays `Orrery` on disk even
  though the crate is now `canvas`. Renaming it is a format migration, already parked as
  a separate vocabulary decision. Do not sweep it with the names.
- Meerkat's scenario runner as code; the VOCABULARY (one description, two
  runners) returns as Action-driven automation per the native-automation
  plan.

## The toolkit question, deferred not answered

Recorded so it stops being decided by omission.

Merecat has no view framework in its dependency graph. Rung 3's chrome hand-builds a
`ScriptedDom` and, per its own module doc, "rebuilds wholesale per state change rather
than diffing". That was the right call for a seven-entry palette and it shipped well. It
will be tested by a roster with hundreds of rows and clickable facet cards. That pressure
is the trigger for this decision.

The alternative is `cambium` (the extracted xilem fork in repos/cambium: packages
`cambium`, `meristem`, `sprigging`, `cambium-nematic`, `cambium-winit`), which supplies
button, checkbox, radio_group, select, slider, text_field, editor, menu, overlay,
data_grid, Keyed and PortableKeyed, and a multi-window runner, with 181 tests green.
Note the flagship component (the filterable action list) does **not** exist in it:
`menu()` is render-only and the host owns query, selection, and keyboard. Do not budget
it as supply.

Merecat is the only family member currently insulated from the toolkit churn, because it
took no toolkit dep. That insulation is an asset. Spend it deliberately.

## Watch items

Neither is a dependency; both landed after the 2026-07-10 draft.

- **The Cambium duplication is live.** Cambium's C5 step (delete the copies in genet) has
  not run: `components/xilem-serval`, `components/xilem-core`, and `components/chisel` are
  still genet workspace members, so two diverged copies of the toolkit exist. mere's HEAD
  (`271e79d`, "Point Meerkat UI at Cambium") already points meerkat at cambium through its
  gitignored `.cargo/config.toml`, which means mere's committed manifest builds only because
  of a local file. This is the thing that gates rung 5's pelt-desktop renderer option, and it
  is not merecat's to fix.
- **`livery`.** `livery` 0.0.2 and `genet-livery` 0.0.2 are genet workspace members at HEAD:
  a clean-room, generated CSS property and cascade engine. `genet-livery` deps
  `layout-dom-api`, `paint_list_api`, and taffy, the same seam `ui.rs` already drives through
  genet-layout. Genet's own audit names Cambium as first consumer and says it "does not claim
  to be Merecat's production theme". Watch, do not plan against it; the swap would be confined
  to `ui.rs`'s layout and paint call.

## First slice (2026-07-10)

Restructure the existing bin onto the spine, behavior-preserving:
`action.rs` (Action/Effect/Update), `app.rs` (state + update), `web.rs`
(the fetch port + enrichment stamps), `session.rs` (the persistence port),
`shell.rs` (winit + present + input mapping + the effect runner), `main.rs`
(a page of bootstrapping). Done when: the same behaviors hold (open address,
fetch enrichment, favicon, session restore, canvas input), verified by build
plus a headed smoke on the scratch profile, with every mutation path already
flowing through `Action`.

This plan supersedes the founding doc's target-shape section (which is now
amendment on amendment); the founding doc keeps the naming, sequencing, and
done-conditions story.

## Progress

- 2026-07-10: Plan written (with Mark: "obviate meerkat, architect merecat";
  the inker/genet migration explicitly belongs to the parallel agent).
- 2026-07-10 (same session): **First slice LANDED** (a8d7117): main.rs is a
  page of bootstrapping over action/app/browse/session/shell; keys,
  close-save, boot fetch, and enrichment flow through the spine;
  headed-smoke verified on the scratch profile.
- 2026-07-10 (same session): **Rung 3 first slice LANDED**: the summonable
  omnibar over the layered present. Ctrl+L / Ctrl+K summon (Ctrl+K seeds the
  `>` lane, a hint row this slice); typing flows through Omnibar Actions;
  the find lane matches graph nodes (label/host/url, recency-ranked) ahead
  of the go row (address-shaped input, https:// for dotted hosts); commit
  selects an existing node without refetching or lowers to OpenAddress. The
  chrome layer is merecat's first DOM surface: a ScriptedDom laid out by
  genet-layout into a paint list, rasterized to a transparent-cleared texture and
  alpha-composited above the canvas texture (the minimal layered-present
  seam, in code). First-run bare launch auto-opens the palette; a bare
  relaunch restores quietly. Placement lesson: genet-layout positions
  absolutes by transform-translate (the gnode path), not left/top. Headed
  receipt: `testing/merecat/images/2026-07-10_omnibar_centered.png` (typed
  "meer", the restored Wikipedia node amber-highlighted). Next slices: the
  `>` actions lane over the Action registry, at-rest focused-node caption,
  IME/caret honesty.
- 2026-07-10 (same session): **Second-agent review folded in.** Adopted now:
  port-agnostic Update messages (the fetch adapter converts at the
  boundary), the gesture law, `web` renamed `browse` with `content`'s
  charter fixed, the crate-promotion gates, the behavioral-receipt deletion
  matrix, the minimal layered-present seam scheduled for rung 3, and the
  pane-gate correction. Recorded with triggers: snapshot/events (first
  automation/scenario consumer; a11y projection pulled OUT of the long
  tail), action envelopes + ingress authorization (first non-local source),
  the multi-window shape (pooled canvas per graph + viewport install — the
  existing seam, not a canvas rewrite), async boot/shutdown (when
  persistence goes async), node-id correlation for enrichment (next
  enrichment touch; request ids with the content lane).
- 2026-07-10 (same session): **Rung 3 second slice LANDED**: the `>` actions
  lane over a `palette_actions()` registry in action.rs (six entries this
  slice); committing an Act row lowers the registry Action through the same
  `update` spine as everything else. At-rest caption: a `.whereami` chip
  (focused node's display label + host) bottom-left whenever the omnibar is
  closed, so the layered chrome earns its keep between summons. Click-away
  closes the palette; plain-key summons `/` and `>` beside the Ctrl chords
  (chord-free, so synthesized-input drivers cannot lose the modifier race,
  which SendKeys demonstrably did with Ctrl+K). Receipts: unit (6 tests:
  the registry filter, both commit paths, find-before-go, normalization,
  hints) and headed (`rung3-actions-lane2.png`: `>re` filters to "Reseed
  layout" selected + "Toggle height-by-degree"; `rung3-boot-framed.png`).
  Boot-framing fix landed WITH the slice: the headed run exposed that a
  restored session whose persisted positions settled away from the origin
  boots to empty ground (recenter frames the origin, not the content).
  `Canvas::fit_to_content()` added in the canvas crate (the
  fit-to-content_bounds camera recenter's doc promised; zoom fits padded
  bounds, capped at 1.0, empty graph falls back to recenter; 2 tests) and
  merecat's `resumed` now calls it. Remaining rung 3 polish: IME/caret
  honesty.
- 2026-07-11: **Self-drive scenario lane LANDED** (the vocabulary's first
  automation consumer; this fires the recorded snapshot/events trigger, so
  the observation pair is now next-up material). `MERECAT_SCENARIO` points
  at a script (grammar: open / omnibar / type / key / act / settle /
  capture / assert omnibar|focused|suggestions|visible / log); the shell
  pumps one step per rendered frame; every step lowers to an ordinary
  Action through the same spine as a keypress, and `act <label>` commits a
  `palette_actions()` registry entry, so the runner adds no second
  execution model (doctrine 2 held). Self-capture composes the frame's
  OWN presented layer views into a COPY_SRC target and reads back (never a
  re-rasterization); `scenario.done` carries `RESULT ok|fail` + log, the
  same sentinel `Run-Scenario` parses; the run never saves the session, so
  scenarios are rerun-deterministic. Harness twin: `Run-MerecatScenario`
  in mk-harness.ps1 (per-run fresh profile by default). Seed:
  `scenarios/rung3_omnibar.scn` (find lane, caption, actions lane,
  isometric; RESULT ok, 5 captures, all asserts green). This lane is why
  the day's other finding surfaced: capture-state tracing proved the
  palette OPEN in app truth while the pixels lacked the card, and a
  paint-command probe pinned an in-flight genet-layout regression (the
  SECOND absolutely-positioned sibling's subtree emits no paint; genet
  checkpoint c80c78d) — which also blanks the canvas gnode pool, and
  retroactively explains part of the SendKeys "lost key" confusion.
  Canary: `ui::tests::chrome_absolute_siblings_all_paint`, `#[ignore]`d
  until the genet lane lands (run with `--ignored` to re-check). Pixel
  receipts for the card re-run through the scenario the moment that fix
  lands; the lane itself is proven.
- 2026-07-11 (same session): **The paint drop FIXED genet-side** (genet
  f7b3c53), with Mark's authorization. Not a recent regression after all:
  genet-layout had never supported multi-root documents (a host DOM with
  no `<html>` wrapper) — build_box_tree took only the document's first
  element child, run_cascade traversed only the first, and the
  root-background propagation promoted the first sibling's background to
  the whole canvas. Merecat's chrome (chip + card as document-root
  siblings) and the canvas gnode pool were the first two-root consumers.
  Fixed: synthetic block root over all document-level elements, cascade
  loops every root, root-background gates on a sole root; 295/295
  genet-layout tests green + a multi-root regression test. Merecat's
  canary un-ignored and green. The scenario now delivers the full pixel
  receipt: `04_actions_lane.png` shows the centered card (`>re`, Reseed
  layout selected), the whole sample graph WITH node bodies (also this
  bug), and the caption pill. The scenario lane found, isolated, and
  verified this end to end — the receipt honesty paid off on day one.
- 2026-07-11 (same session): **Rung 3 COMPLETE — IME/caret honesty
  landed.** The omnibar gets a real caret: `cursor` (byte offset,
  char-boundary safe) with Left/Right/Home/End motion, Delete, and
  backspace/insert AT the caret; the input line renders split at the
  cursor so the block caret draws at its true position. New vocabulary:
  `OmnibarInsert(String)` (the IME-commit path, and later paste),
  `OmnibarDelete`, `OmnibarCaret(CaretMove)`. IME: the shell enables the
  window IME on omnibar open/close transitions, aims the candidate
  window at the caret's neighborhood (`ui::ime_cursor_area`, average-
  advance approximation), shows the in-flight preedit underlined at the
  caret, and lowers only the commit through the spine — preedit rides
  directly on state per the gesture law (composition is ephemeral; the
  commit is the semantic event). Scenario grammar grew `insert`, the
  caret keys, and `assert text`; receipts: 12 unit tests +
  `02b_caret_mid_text.png` (caret drawn mid-text after two Lefts). The
  ladder's next rung is 4: live content through the engine registry
  (inker adoption landed genet-side, so it is unblocked).
- 2026-07-12: **Correlation-over-URLs LANDED for the browse lane** (the
  recorded trigger fired at rung 4's enrichment touch; the content lane
  already resolved by member). Fetch effects carry the requesting node's
  id; the shell's `browse::PendingFetches` notes it before commanding the
  actor and the adapter reattaches it on completion — the fetch crate's
  wire types are untouched (meerkat shares them; same-URL requesters are
  interchangeable, so the table keys by URL and pops one per completion).
  Stamps land on the exact requester via new mere-side member-keyed
  setters (`set_node_title_for`/`set_node_mime_hint_for`/
  `set_node_favicon_for`); a late result against a node that navigated
  away drops explicitly. Receipts: URL-twin targeting, superseded drop,
  refuse-to-guess on unmatched completions.
- 2026-07-12 (same session): **The observation pair LANDED** (recorded
  trigger: the scenario lane is the first automation consumer).
  `observe::snapshot(app)` is one coherent read (focused node, omnibar
  view with suggestion rows as display strings, per-node content
  lifecycle, node count, graph visibility); `observe::AppEvent` is the
  stream half, emitted at the semantic tier (address-opened,
  omnibar-opened/closed/committed, layout-reseeded, content states) and
  drained by the shell each frame. Scenario asserts now read the
  snapshot instead of poking app fields — a green scenario certifies the
  surface the a11y/automation lanes stand on — and `assert event
  <substr>` covers the stream; a failed run's sentinel carries the event
  tail as its diagnosis. Gesture-end events (click-selection,
  drag-placement) stay with the gesture-law follow-up, noted in
  observe's charter. 20 unit tests; both scenarios RESULT ok.

  *(Chronology note: the rung-4-prep entry below is dated 2026-07-11 and sits after the
  two 2026-07-12 entries. It is left in place as accurate history, but "the last entry"
  is not the current state. That mis-ordering is precisely how rung 4's landing went
  unrecorded for three days. Newest entries go at the bottom from here on.)*
- 2026-07-11 (same session): **Rung 4 prep — the `content` module is
  born**, sized to meet the session-engines plan (genet docs 2026-07-10)
  at its phase 2/4 boundary. App truth: `ContentStates` (node id ->
  Requested/Live/Failed lifecycle; failure is a surfaced, retryable
  state, per the no-placebo rule). Vocabulary: `Action::ToggleNodeContent`
  (flips the focused node, in the palette registry), effects
  `SpawnContent`/`CloseContent`, updates `ContentSpawned`/`ContentFailed`.
  The shell's effect runner holds the port SLOT: sessions are retained
  non-Send handles, so they will live shell-side keyed by node id; until
  genet-documents lands the port answers every spawn with an honest
  failure naming the gap. When phase 2 lands, the wiring delta is
  confined to that runner arm plus frame composition into the layered
  present — the vocabulary, lifecycle, palette entry, and scenario
  drivability are already in place. 15 unit tests.

  *(Superseded 2026-07-11 by d1e6234, below: the port no longer answers every spawn with
  an honest failure. The rest of this entry is accurate history.)*
- 2026-07-11: **Rung 4 LANDED for the static lane** (d1e6234). The reserved runner arm
  went live: `Effect::SpawnContent` builds an `inker::EngineRouteRequest`,
  `mere::routing::route_policy()` picks the engine id, `inker::SessionRegistry` spawns,
  and the retained non-Send `Box<dyn DocumentSession<netrender::Scene>>` lives shell-side
  keyed by node id. Ports own handles, App holds data, exactly as prepped. Static lane
  only (`StaticSessionEngine` registered under `genet.web`, with the shell's
  `LocalFetcher`). Receipt: `scenarios/rung4_content.scn`, `assert content-live`,
  RESULT ok. Still open on rung 4, and now scheduled into rung 5 because each needs the
  surface plan: content input routing (zero calls to `click_at`, `scroll_by`, or `links`);
  per-node placement (the session frames at full window and composites opaque, so it
  occludes the canvas); non-focused nodes (only `focused_member()` is pumped and framed,
  so `ContentStates::live_nodes()` is dead and `Live` is a lie for every other node); the
  compat lane (no `scrying-engine` dep; `verso-tile` IS in the graph through `fetch`); and
  scripted/smolweb (a `genet-documents` feature flip plus two register calls).
- 2026-07-12: **Ring-3 fork pins mirrored** (fa41d4c) and **Genet consumed** (37e536f):
  the engine's rename from Serval landed in merecat's manifest.
- 2026-07-14: **The rename completed under us, and merecat came through green.** The local
  folder is now repos/genet (it was repos/serval through 07-13), `serval-xilem` is package
  `cambium`, and chisel is package `sprigging`. Merecat's gitignored `.cargo/config.toml`
  was swept onto repos/genet and its `serval-*` toolkit patch keys replaced by a cambium
  section. Receipt: `cargo metadata --offline` exits 0 and `cargo check --bin merecat` is
  green (2m08s, one warning, the dead `live_nodes` noted above). There was a real but
  transient break mid-sweep: genet moved onto a published `genet-stylo` registry family
  while merecat's lock still pointed at a stale genet rev, which put two `genet-stylo`
  copies in the graph against one `links = "servo_style_crate"` key. It resolved when the
  sweep finished. Watch for it if a sibling's stylo pins move again.
- 2026-07-14: **Rung 5 scoped** in
  [2026-07-14_merecat_rung5_panes_plan.md](./2026-07-14_merecat_rung5_panes_plan.md), and
  this plan refreshed against it. The headline correction: platen is not a pane model (every
  `Workbench` mutator takes a `forme::GraphMemberId`), the pane model is mere's `frisket`
  crate, and the surface composition + focus routing seam this plan booked as born-at-rung-3
  was never born. Rung 5's first slice builds it.
- 2026-07-14: **The pane model renamed and split** (with Mark), which answers rung 5's
  highest-leverage open question before it was asked twice. `frame` was two crates in one coat:
  mere's workspace identity vocabulary (`GraphId`, `SessionId`, depended on by `crawl` and
  `session-runtime`, with nothing to do with panes) fused with the pane model. Cut at that seam.
  **`incipit`** (new, `crates/incipit`, serde + uuid only) holds the ids and stays in mere; it is
  what lets `crawl` name a graph without depending on `kernel`. **`frisket`** (renamed from
  `frame`, `crates/shell/frisket`) is the pane model; a frisket is the press frame whose cut-out
  apertures decide what prints where, and it sits beside `platen` and `forme` in the press
  anatomy. `frame` was overloaded three ways here (a rendered frame, a `TileFrame`, a window's
  pane arrangement), which is why it went. `session_runtime::frame_layout_store` became
  `frisket_store`. Frisket's destination is merecat (its `PaneContent` names panes merecat owns
  outright, and a library crate enumerating the app's panes is an inversion), but it cannot move
  while meerkat depends on it, so merecat git-deps it exactly as it already does `fetch` and
  `session-runtime`, and it relocates at meerkat's deletion with `frisket_store` alongside. Two
  names deliberately left on disk, to change together in one format migration rather than as two
  silent breaks: the sidecar is still `frame.json`, and `PaneContent::Orrery` is still `Orrery`.
  Receipts: frisket 14 tests, incipit 3, session-runtime 188, meerkat compiles green.
- 2026-07-16: **Piccolo control scripting is integrated as a host seam.** Merecat's
  opt-in `piccolo` feature wires Genet's `script-engine-api` and
  `script-engine-piccolo` through `src/script.rs`. Four tests cover read-only
  snapshots, typed navigation/pane/session Actions, capability denial, and the
  step budget. The default build remains Piccolo-free. This does not claim page
  JavaScript, arbitrary graph mutation, chrome extension registration, or a
  user-facing workflow command; those remain explicit follow-on surfaces.
- 2026-07-17: **The follow-on surfaces above now have a canonical cross-repo
  plan**: the participant gate + packs plan
  (`mere/design_docs/mere_docs/implementation_strategy/2026-07-17_participant_gate_packs_plan.md`,
  designed with Mark). One authority gate for every non-UI actor (script, wasm
  component, moot peer, agent, scenario runner); merecat's stake is doctrine 2
  extended (proposals are typed Action mirrors, never strings), the palette
  populated from participant nested graphs, and a typed merecat WIT world as
  that plan's B3. The resident helper unit is named **servitor** (reserved on
  crates.io); chartulary's nesting substrate (B0) landed the same day.
