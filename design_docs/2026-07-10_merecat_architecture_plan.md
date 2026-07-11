# Merecat architecture: obviating meerkat

2026-07-10. The founding doc set the role swap (mere = library, merecat = the
reference host); the boundary pass sharpened what mere keeps. This plan is
the other half: what merecat IS, structurally, and the ladder by which
meerkat becomes unnecessary. Companion to
[2026-07-08_merecat_founding.md](./2026-07-08_merecat_founding.md) and mere's
2026-07-09 boundary pass plan (its decisions are assumed here, as amended:
canvas stays mere, platen is the pane home and arrives with the port, the
verso family is serval's `verso-tile`).

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
| `content` | engine registrations, per-node document lifecycle, verso-tile flip, content frames, input routing (the registry itself is serval/inker's) | absent | rung 4 births it; crate if feature isolation changes the dep graph |
| `session` | persistence port: graph.json now; browser_nodes.json, view intent, settings, multi-session later | landed | multi-session lands |
| `shell` | winit + SurfaceHost + layered present + input routing + effect runner | landed | a second host (wasm) appears, or desktop/web shells share the core |
| `ui` | chrome DOM (omnibar, toolbar), pane tiles over platen | absent | it exists at all (needs xilem-serval + platen) |

Crate promotion is gated on consumers and build shape, never on module size:
headless automation wanting `action + app`, a second shell, another app
consuming a port, or feature isolation that changes the dependency graph.
The likely eventual shape is `merecat-core` / `merecat-desktop` /
`merecat-web`; until a gate fires, modules are correct.

Rung 3 also births the **layered present seam** in minimal form: the shell
composites an ordered list of surfaces (canvas, then the chrome layer) with
rects and focus-routed input, and that seam grows into a full surface plan
(ids, z-order, per-surface input routing) at rung 5 when panes and content
frames multiply. `Shell::render` must never become the next imperative
crossroads.

mere::canvas is hosted, not wrapped: the shell maps raw input onto the
canvas's semantic methods directly (they are already the right vocabulary),
and canvas-affecting Actions call them from `update`'s effect handler. A
merecat-side canvas facade would be indirection with no second consumer.

## The obviation ladder

Meerkat dies when merecat is the daily driver. Each rung names the meerkat
capability, its source of supply, and its gate. Rungs are ordered by
daily-driver value, not by meerkat's module sizes.

1. **Graph canvas + persistence + enrichment** — mere::canvas, fetch,
   session-runtime. LANDED (slices 1-4, 2026-07-09): open address, fetch,
   title/mime/favicon stamps, session restore.
2. **The Action spine** — this plan's first slice: restructure the bin onto
   action/app/web/session/shell modules, behavior-preserving. Gate: none.
3. **Omnibar + navigation chrome** — shell/chrome's host-neutral vocabulary
   (History, NavTarget, suggest) over a xilem-serval chrome document, as a
   second composited layer above the canvas. Gate: none (the vocabulary and
   the render stack both exist). This is the rung that makes merecat usable
   without a terminal argument.
4. **Live content on nodes** — the engine registry via the inker adoption;
   pelt's Engine impls; document scenes for readable pages; scry tiles +
   verso-tile flip for the compat lane. Gate: the mere->serval agent's
   migration (adoption plan done-conditions 1-6).
5. **Panes** — platen (the pane home) + the domain crates (gloss, roster,
   trail, alembic/steward over session-runtime). Gate: the surface
   composition + focus routing seam (born minimal at rung 3) plus platen's
   pane model; the chrome document is a product-order choice, not the
   structural gate.
6. **Multi-session + browser-state sidecar** — session-runtime's manifests,
   sessions/<id>/ layout, browser_nodes.json. Gate: rung 4 (per-node browser
   state is worth persisting once nodes hold live content).
7. **Multi-window** — the one-state-N-windows doctrine over the Action spine
   (a window is a projection, so the spine already has the right shape).
   Gate: rung 3.
8. **The long tail** — comms (murm posture), intel (embed/infer glue),
   import/crawl, scripting (rhai + document-host), theming
   (register-theme/tinct), a11y projection. Each is a port + Actions; each
   arrives when wanted, none blocks obviation of the daily-driver set.

**Meerkat's deletion condition** (the founding doc's done-condition 2 made
concrete; behavioral receipts, not a subjective trial — 2026-07-10 review
round): every row below passes, at which point daily-driving merecat is
confirmation rather than the specification.

- Open addresses from the omnibar; navigate live content.
- Back, forward, reload, and redirects behave.
- Focus and switch between the graph canvas and documents.
- Open, arrange, and restore panes.
- Restore graph, browser state, and content state after a restart.
- Change engine/viewer settings and see them apply.
- Run the same scenario through keyboard input and through automation
  Actions (one description, two runners).
- Produce a coherent application snapshot and accessibility tree.
- Recover from a failed fetch, a failed engine start, and an interrupted
  save.
- Meerkat holds no capability absent from this matrix.

Then meerkat leaves mere's workspace and the mere facade drops its
compatibility re-exports (platen moves here in the same pass).

## What is deliberately NOT carried

- meerkat's `command_drain` / `shell_eval` execution model (replaced by the
  Action spine; the command REGISTRY vocabulary is worth rereading when rung
  3 adds a command palette).
- The bin-module sprawl (85 modules in main.rs). Merecat's bin stays a shell.
- `PaneContent`/`ContentPane` vocabulary as-is; pane identity gets designed
  at rung 5 against platen's model, not inherited.
- Meerkat's scenario runner as code; the VOCABULARY (one description, two
  runners) returns as Action-driven automation per the native-automation
  plan.

## First slice (this session)

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
  the inker/serval migration explicitly belongs to the parallel agent).
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
  serval-layout into a paint list, rasterized to a transparent-cleared texture and
  alpha-composited above the canvas texture (the minimal layered-present
  seam, in code). First-run bare launch auto-opens the palette; a bare
  relaunch restores quietly. Placement lesson: serval-layout positions
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
  paint-command probe pinned an in-flight serval-layout regression (the
  SECOND absolutely-positioned sibling's subtree emits no paint; serval
  checkpoint c80c78d) — which also blanks the canvas gnode pool, and
  retroactively explains part of the SendKeys "lost key" confusion.
  Canary: `ui::tests::chrome_absolute_siblings_all_paint`, `#[ignore]`d
  until the serval lane lands (run with `--ignored` to re-check). Pixel
  receipts for the card re-run through the scenario the moment that fix
  lands; the lane itself is proven.
- 2026-07-11 (same session): **The paint drop FIXED serval-side** (serval
  f7b3c53), with Mark's authorization. Not a recent regression after all:
  serval-layout had never supported multi-root documents (a host DOM with
  no `<html>` wrapper) — build_box_tree took only the document's first
  element child, run_cascade traversed only the first, and the
  root-background propagation promoted the first sibling's background to
  the whole canvas. Merecat's chrome (chip + card as document-root
  siblings) and the canvas gnode pool were the first two-root consumers.
  Fixed: synthetic block root over all document-level elements, cascade
  loops every root, root-background gates on a sole root; 295/295
  serval-layout tests green + a multi-root regression test. Merecat's
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
  (inker adoption landed serval-side, so it is unblocked).
