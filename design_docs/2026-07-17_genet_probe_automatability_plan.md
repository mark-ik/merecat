# genet-probe: shared automatability for the genet apps

**Status:** design spike (no code yet). **Crate home:** genet (engine layer),
consumed by every genet app. **Lineage:** grows out of merecat's self-drive
harness + observation stream; sibling to
[`2026-07-15_merecat_surfaces_in_cambium.md`](2026-07-15_merecat_surfaces_in_cambium.md).
**Working name:** `genet-probe` (naming is Mark's call — see Open questions).

## Why

The question this answers: across the genet apps (merecat, isometry, woodshed,
hocket), do we meet the baseline for **diagnostics, accessibility, and
instrumentality** that makes an app automatable — testable, but also legible and
drivable by scripts and models — and what would closing the gap require?

The survey (2026-07-17, against the actual repos) found a lopsided answer:

- **The substrate is shared and strong.** Every genet app is cambium-based, so
  every app emits a semantic ARIA-attributed `ScriptedDom` (`role`,
  `aria-selected`, `aria-label`, `tabindex`). genet-layout gives all of them
  `hit_test` (point -> node), `absolute_rect` (node -> rect), and
  [`a11y.rs`](../../genet/components/genet-layout/a11y.rs)'s DOM -> accesskit
  `TreeUpdate` projection. "Click the element labelled X" and "what does the
  a11y tree say" are **already answerable for any cambium app** — that is the
  whole reason merecat's scripting is ask-the-layout, not pixel-poking.
- **The patterns are proven once and shared zero times.** The typed observation
  stream (`AppEvent` + `assert event`), the loud-and-attributable divergence
  events, and the self-drive harness (`MERECAT_SCENARIO`, ~20 verbs, `RESULT
  ok/fail` sentinel) all live in [merecat/src/scenario.rs](../src/scenario.rs)
  and [observe.rs](../src/observe.rs) — app-local, ~80% generic, portable
  nowhere. isometry and woodshed have no harness at all; hocket's `-headless`
  binary is a hardcoded audio demo, not a scriptable driver.

So the move is not to build harness+diagnostics into four apps four times. It is
to **extract the generic 80% into a shared genet crate** that every cambium app
adopts through a small trait, with merecat as the first consumer and validator.

### The per-axis picture

| Axis | merecat | isometry | woodshed | hocket |
| --- | --- | --- | --- | --- |
| Semantic DOM / ARIA (cambium, shared) | yes | yes | yes | yes |
| accesskit tree (genet-layout, shared) | yes | yes | yes | yes |
| Leaf a11y (sprigging custom-paint) | partial | partial | stub | stub |
| Diagnostics (typed events + snapshot) | reference | none | none | none |
| Loud+attributable divergence | yes (07-17) | none | none | none |
| Self-drive harness | full | none | none | demo only |

## What is generic vs app-specific

The extraction lives or dies on this cut. Everything that stands on
genet-layout + the semantic DOM is generic; only what needs app-typed state or
app-specific routing crosses into the trait.

**Generic (the crate owns it):**

- The scenario-file parser and the verb dispatch loop.
- Semantic resolution: "the element with `role=tab` whose text contains `Links`"
  or "the `.list-row` containing `example.com`" -> a window-space point, via
  `absolute_rect` over the app's surfaces. This is exactly merecat's
  `tab_center` / `row_center` / `node_center`, generalized off the specific
  panes.
- DOM asserts: `assert role <role> <label>`, `assert text <substr>`,
  `assert visible <selector>`.
- The `RESULT ok/fail` sentinel, the log, capture acknowledgment.
- The divergence convention: `interaction-missed` / `affordance-unavailable`
  emitted when a resolution finds nothing (merecat landed this 07-17; the crate
  makes it every app's).

**App-specific (the trait `Automatable`, the small surface an app implements):**

```rust
/// One retained cambium surface the driver can search and hit-test: its DOM,
/// where it sits in the window, and the sheet it lays out under. An app with
/// several retained runners (merecat: chrome + roster grid + gloss + trail)
/// returns one entry each; the driver resolves a selector across all of them.
pub struct ProbeSurface<'a> {
    pub name: &'static str,        // "roster", "chrome", ...
    pub dom: &'a ScriptedDom,
    pub rect: [f32; 4],            // window-space [x, y, w, h]
    pub sheet: &'a str,
}

pub trait Automatable {
    /// The hit-testable surfaces, this frame. The driver lays each out and
    /// resolves selectors against all of them — so "click-row X" needs no
    /// per-pane method, unlike merecat today.
    fn surfaces(&self) -> Vec<ProbeSurface<'_>>;

    /// Typed observation for asserts the DOM cannot express: focus, the pane
    /// tree, a split ratio, counts. The app's existing snapshot shape.
    fn snapshot(&self) -> ProbeSnapshot;

    /// Drain the semantic events since the last call, as describe-strings
    /// (`assert event` matches substrings). The app's existing event stream.
    fn drain_events(&mut self) -> Vec<String>;

    /// Run one app-named command — the `act <label>` verb (merecat's palette
    /// actions). `false` if no such command, so the driver fails loudly.
    fn act(&mut self, label: &str) -> bool;

    /// Deliver a synthetic pointer event at window coords. The app routes it
    /// through its own surface plan / capture (that routing is app-specific;
    /// the driver only supplies the point it resolved).
    fn press(&mut self, x: f32, y: f32);
    fn moved(&mut self, x: f32, y: f32);
    fn release(&mut self, x: f32, y: f32);
}
```

`ProbeSnapshot` starts as the small shared subset every app can answer (focused
label, a string map of named counts/flags) and grows by need; app-only asserts
can also go through `drain_events` rather than bloating the snapshot.

Note the payoff hiding in `surfaces()`: because the driver resolves a selector
across *all* retained DOMs uniformly, the extraction **simplifies** merecat —
`tab_center`, `row_center`, `node_center`, `click_pane_row`, `click_pane_tab`,
`click_pane_node` collapse into one generic resolver. The app stops owning
per-widget geometry lookups; it just lists its surfaces.

## The verb vocabulary (shared)

Carried over from merecat, made selector-driven so they are app-agnostic:

- `act <label>` — run an app command.
- `click <role|.class> <text>` — resolve across surfaces, press+release at the
  centre. Subsumes `click-row` / `click-tab` / `click-node`.
- `drag <from> <to>` — press, move, release (the divider gesture).
- `key <named>` / `type <text>` — keyboard.
- `assert text <substr>` / `assert role <role> <label>` — DOM asserts.
- `assert event <substr>` — the semantic event stream, including divergence.
- `assert snap <field> <op> <value>` — typed observation.
- `settle <n>` / `capture <name>` / `log <text>` — pacing, receipts.

A miss on any `click`/resolve emits `interaction-missed <selector>` into the
event stream (not just stderr), so a receipt that drives a miss fails instead of
green-lighting it — the property merecat just proved with `rung5_divergence.scn`.

## The accessibility follow-on (separate, smaller)

The one place the accessibility baseline genuinely is not met is
**sprigging custom-paint leaves**: the graph canvas, meters, woodshed's
fretboard/chord views, hocket's waveforms mostly leave their
`fn accessibility(&mut accesskit::Node)` hook empty, so pixel-painted content is
invisible to AT and to any a11y-tree-based automation. (The graph canvas is the
partial exception — it overlays real DOM node-buttons, which is why the minimap
is already scriptable.) This is orthogonal to the harness extraction and can
land per-leaf, per-app; it belongs on each app's list, not this crate's. Flag it
here so it is not mistaken for something `genet-probe` covers.

## Sequencing

1. **Extract, with merecat as the reference.** Lift the generic 80% of
   `scenario.rs` into `genet-probe`; leave merecat's `Automatable` impl behind
   (its surfaces, snapshot, event drain, act, pointer routing). merecat's
   existing scenarios must pass unchanged — that is the extraction's receipt.
   The per-pane geometry methods collapse into the shared resolver.
2. **Second consumer proves it is really generic.** Adopt in one more app —
   isometry is the natural pick (already a `data_grid` production consumer, so
   its DOM is rich). A single scenario driving isometry's grid, written against
   the shared verbs, with zero new harness code, is the proof.
3. **Diagnostics parity.** Give isometry/woodshed/hocket the small
   `Automatable` surface (snapshot + event drain). Most already have app state
   to project; the work is the adapter, not new machinery.
4. **Leaf a11y** (separate track, per app) as capacity allows.

## Open questions

- **Name.** `genet-probe` is a working handle (probe = observe + drive). Plain
  infra name by the plain-vocabulary rule; Mark's call. Alternatives:
  `genet-drive`, `genet-harness`, `genet-legible`.
- **Doc home.** This plan sits in merecat's design_docs (reference consumer,
  established flat convention, lineage with the surfaces doc). If genet grows
  its own `design_docs/`, the crate's technical-architecture doc goes there and
  this stays the merecat-side driver record. Cross-linked either way.
- **Snapshot shape.** Start minimal-shared and grow, or define a fuller common
  `ProbeSnapshot` up front? Lean minimal — the event stream absorbs most
  app-specific asserts, and a bloated shared snapshot is a coupling smell.
- **Scope of `act`.** merecat's `act` runs palette actions. Is "run a named
  command" the right universal verb, or should the driver reach app intents more
  structurally? Palette-label is stringly-typed but matches how a human or model
  would name the action, which is the automatability goal.

## Finding: a widget is only genet-probe-resolvable if its identity is in the DOM

Wiring merecat's first verb surfaced the sharp edge of the whole idea. The
resolver can only find what the **semantic DOM** carries — a selectable class
(or role) plus the target text reachable as the element's own text or its
`aria-label`. Against merecat's four widget shapes:

- **Tab strip** — resolvable. `.tab` with the label as direct child text. Wired.
- **Sectioned list (Trail)** — resolvable. `.list-row` with direct child text.
  Ready to wire.
- **data_grid (Roster rows)** — NOT yet. The clickable cell is a bare `<span>`
  (no class) and its text sits one level below the `.grid-cell` wrapper, so a
  `.grid-cell` selector with shallow text matching misses it, and a deep match
  would first hit the enclosing `.grid` container. Needs a small cambium change:
  give the grid cell (or its clickable) a stable class with reachable text.
- **graph-canvas node (Gloss)** — NOT yet, for a different reason. The scenario
  targets a node by **url**, and the url is only in pane state, not the DOM (the
  node button's `aria-label` is the display label). The node needs its url in
  the DOM — a `data-url` attribute or the aria-label — before a url-selector
  resolves it.

This is the automatability baseline made concrete at the widget level: **an app
is drivable exactly to the extent its targetable identity lives in the semantic
DOM.** It sharpens the accessibility axis too — the same DOM identity a driver
needs is what an AT tool reads. So the follow-on is not just "wire the other
verbs"; it is "put the missing identity in the DOM," which is a cambium/app fix
each of those two widgets wants regardless.

## Progress

- 2026-07-17 — Spike written. Prior same-session groundwork that makes this
  cheap: merecat's divergence events landed (`94d685a`), and the four catalog
  components this crate's verbs resolve against (tab strip, split, sectioned
  list, the graph-canvas leaf) are all in cambium with ARIA semantics.
- 2026-07-17 — **Slice 1: resolver founded** in genet (`genet-probe`,
  `ProbeSurface` + `Selector` + `resolve` + `text_present`, 5 tests, MIT/Apache).
  Proven to resolve within merecat's dependency graph via the local patch (genet
  main is 12 commits ahead of origin with foreign work, so `genet-probe` is NOT
  yet on origin/main — a clean merecat checkout needs it pushed there; deferred,
  not forced with foreign commits in the way).
- 2026-07-17 — **Slice 2 (partial): merecat is the first consumer.** `click-tab`
  routes through `genet_probe::resolve`; `RosterGrid::tab_center`'s bespoke
  geometry collapsed to a 3-line `resolve` delegation. Unit tests + the roster
  and divergence scenarios green through the shared crate. `click-row` /
  `click-node` await the DOM-identity fixes above before they collapse too.
