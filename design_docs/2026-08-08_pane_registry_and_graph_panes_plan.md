# Pane Registry and Graph Panes Plan

**Date**: 2026-08-08
**Status**: Direction settled; nothing implemented. Originated as a read-only
review by one agent, verified against the tree and amended by a second, with
the graph-as-pane reframe added by Mark. Every file citation below was checked
against the code on 2026-08-08, not carried from the review on trust.

**Scope**: Generalize the pane machinery behind a registry, make pane
instances genuinely independent, and make the graph canvas a pane like any
other so one window can split across two graphs. Keep the semantic panes
distinct.

---

## 1. The live problems, verified

1. **Pane instances are not independent.** `summon_pane`
   ([src/app/pane_arms.rs:69](../src/app/pane_arms.rs)) mints a fresh `PaneId`
   per summon, but the shell retains one renderer per kind
   ([src/shell/mod.rs:195](../src/shell/mod.rs): `roster_grid`, `gloss_pane`,
   `trail_pane`, `inspector_pane`, ...), and dispatch is by content kind, not
   id ([src/shell/render.rs](../src/shell/render.rs), `pane_scene_by_kind`).
   Two same-kind panes share one selection, scroll, and control state.

   One nuance the review missed, and the code states outright: the shared
   runner is deliberate for tear-out. "The runner being shared is what makes
   tear-out identity-preserving." Keying by `PaneId` keeps that property,
   because the id survives the move; it fixes only the two-same-kind case.
   This plan refines that decision rather than overturning it.

2. **Binding is a boolean.** `follows_active_graph()`
   ([src/panes/mod.rs](../src/panes/mod.rs)) is yes/no, and the render path
   receives `PaneContent` without the leaf's id or a graph id. The model
   claims graph-scoped leaves without a binding to resolve them from.

3. **`PaneContent` mixes four concerns**: renderer kind, data source,
   per-instance configuration (`Gloss(PaneComposition)`), and extension
   dispatch (`Custom`).

4. **Two parallel enums plus a mapping.** `PaneKind`
   ([src/action.rs:297](../src/action.rs)) maps to `PaneContent` through
   `pane_content()` ([src/app/mod.rs:39](../src/app/mod.rs)); labels derive
   separately. Settings and Publishing already ride magic strings
   (`Custom("settings")`, `Custom("publishing")`), layout ops ride
   `Custom("__placeholder__")`, and `System` is never constructed outside
   tests.

## 2. Graphs are panes

Today `PaneContent::Orrery` is a unit variant. Which graph it shows comes from
the session, one per window; the default layout's own comment calls the leaf
"bound to the nil (unbound) graph, a placeholder"; the summon anchor finds
*the* Orrery leaf, singular. A second graph in the same window is not
unsupported, it is inexpressible: there is nowhere on the leaf to say which
graph.

The reframe: **the canvas is a pane with `binding: Graph(id)` and multiplicity
Many.** A split with two graphs in one window then falls out of the same model
as everything else, and no "primary surfaces" special category is needed.

What other panes key off of becomes a derivation rule rather than a stored
window property:

> The active graph of a space is the graph of its most recently focused
> graph-bound pane.

`ActiveGraph`-bound tools (Roster, Inspector, Trail) follow focus between the
graph panes of a split; a tool pinned with `Graph(id)` stops following. Both
come from the binding enum below; nothing else is needed.

**Doctrine change, recorded deliberately:** this revises "window =
graph-shaped session". The pane becomes graph-shaped; the window becomes a
composition that may view several graphs. Consequences owned by this plan:
`frame.json` persists a `GraphId` on graph leaves (today nothing), and the
summon anchor becomes "the focused graph pane of this space" rather than "the
primary Orrery". Consequences deferred, named in section 9: what a session
means for a multi-graph window, and how Overmap draws one.

Tear-out becomes uniform in passing: tearing out a graph pane yields a window
viewing that graph, which is approximately what a lens window already is.

### The workbench, placed

The workbench is the same graph through the other projection. The canvas
shows the graph as space; the workbench shows chosen members as content:
platen tiles graph nodes inside one leaf
([src/workbench_pane.rs](../src/workbench_pane.rs), whose header says exactly
this), cells composite each member's document as its own surface, tabs are
members. Both projections follow a graph, which is why
`follows_active_graph()` already answers yes for both. Under this model the
workbench takes the same bindings a canvas does: `ActiveGraph` to follow the
space's focus, `Graph(id)` to pin. Two workbenches over two graphs in one
window is as expressible as two canvases.

One boundary stated hard, because unifying it away would be tempting and
wrong: the workbench is a pane, **its cells are not**. A cell's identity is
the member UUID; its arrangement belongs to platen, mere's composition layer,
per the recorded platen/workbench split; it carries no `PaneId`, no binding,
no registry entry. Frisket arranges tools and surfaces; platen arranges a
graph's content inside one of them. Two arrangement systems, on purpose, with
member identity and pane identity kept from competing for one job.

## 3. The model

```rust
PaneRecord {
    id: PaneId,
    kind: PaneKindId,
    binding: PaneBinding,
    config: PaneConfig,
}

PaneDefinition {
    id: PaneKindId,
    display_name: String,
    uniqueness: Uniqueness,
    default_placement: Placement,
    capabilities: PaneCapabilities,
    renderer_factory: RendererFactory,
}
```

`PaneBinding`, explicit: `ActiveGraph` (follows this space's focus, per the
rule above), `Graph(id)`, `Node { graph, member }`, `Session(id)`,
`SessionSet`, `Application`, `Settings(SettingsRef)`, or a typed open source.
Stored bindings resolve against the space they live in; resolution takes the
space as an input.

**Uniqueness names its scope.** A flat multiplicity dodges the question the
one-app-state, N-window model forces: two windows on the same graph both
legitimately want a Roster, so "unique per binding" alone would wrongly focus
the other window's instance.

```rust
enum Uniqueness {
    Many,                 // Canvas, Workbench, Tile, Gloss
    PerSpaceAndBinding,   // Roster, Inspector, Apparatus, Trail
    PerSpace,             // Steward, Comms, Overmap, Settings
}
```

Declared by the registered pane, never inferred from its label. Summoning a
unique pane focuses the existing instance; multi-instance renderers are keyed
by `PaneId`. Tabs and stacks are layout containers, not pane kinds; a tab is a
tile's handle (`TileTab`), per the recorded naming.

The registry retires `PaneKind` entirely: one registration replaces the enum
arm, the `pane_content()` mapping, the palette row, the label derivation, and
the render arm. Layout's `Custom("__placeholder__")` becomes a typed
placeholder at the same time.

**Persistence posture:** `PaneContent` is serialized in `frame.json`. The
repo already has a recorded posture for this migration, in the Gloss
variant's own comment: pre-release, no legacy friction, an unrecognized
layout falls back to the default and logs. This plan adopts that posture by
name. No migration machinery.

## 4. What stays distinct

Consolidate mechanics and shared furniture; preserve the semantic panes:
Roster (what exists), Inspector (what the selection is), Apparatus (how the
object is represented), Trail (where navigation went), Alembic (what memory
persists), Steward (operational activity), Comms (conversation), Gloss
(current-graph projection), Overmap (sessions and lineage), Publishing (a
workflow tool), Settings (addressed configuration). Gloss and Overmap share
projection and section machinery; Roster, Inspector, Apparatus share
furniture; Trail and Alembic share list forms. Semantic merger would make each
group vaguer. Apparatus stays object-facing and does not absorb settings.

## 4b. Chrome, and how configurable it should be

The chrome is the frame around the panes: the omnibar, whatever status the
shell shows, the divider bands, and the topology itself. The direction is that
**as much of it as possible is layout, not code** — the same principle that
makes panes registry entries makes chrome a saved, restorable thing.

### What exists

An omnibar is already here ([src/app/omnibar_arms.rs](../src/app/omnibar_arms.rs)):
one line, opened plain or in a `>` command lane, recomputing suggestions
against the action catalog every keystroke, with `FocusTarget::Chrome` taking
keys while it is open. The catalog it reads is the same one the palette
snapshot and the automation runner read ([src/app/palette.rs](../src/app/palette.rs)),
composed once so the three cannot disagree about what a label means. That
single-catalog property is the asset; the omnibar is a view onto it.

What is missing is not the omnibar. It is **scrollback** and
**configurability**.

### Omnibar with scrollback

The omnibar today is stateless between invocations. Giving it a scrollback —
a retained transcript of commands run and their results — turns it from a
launcher into a shell. The material is already there: `AppEvent`
([src/observe.rs:120](../src/observe.rs)) is the journal every action already
emits, and it is what the automation runner replays. A scrollback is that
journal, made visible and addressable, with command entries interleaved. So
this is surfacing an existing stream, not building a new one.

Design constraints that follow from it being real rather than decorative:
scrollback is per-space (it belongs to a window's work, like the pane tree),
it is bounded (a ring, not an unbounded log), and a result in it is a value
you can act on again, not just text — the same "a result is a value" posture
the Rerun blueprint model takes toward recorded truth. Whether scrollback is
its own pane kind (bound `Application`, `PerSpace`) or a mode the omnibar
expands into is an open question; the pane form composes better with the rest
of this plan, so start there.

### Configurable chrome

The knobs, in rough order of cost:

- **Which chrome is present.** Omnibar shown or summoned, status band on or
  off, palette bound to a key or always docked. These are user settings once
  §6's settings lane interprets `SettingControl` generically.
- **Where it sits.** Omnibar top or bottom, docked or floating. The compositor
  already places surfaces at rects, so chrome placement is the same math the
  panes use.
- **Named layouts** (§7) capture the chrome configuration alongside the pane
  topology, so a saved "Operate" layout can carry a docked scrollback and an
  always-visible Steward while "Browse" carries neither.

Zellij is the closest prior art for the chrome specifically: declarative,
named, restorable layouts where the frame is data. egui_tiles is the model for
the tree being generic over app-owned payloads. Rerun for keeping recorded
truth (the graph, the journal) separate from the saved view of it.

### Floating and nested panes, nested splits

These three are one capability with three names, and they are the reason the
shared-`TileTree` decision in §5 is load-bearing rather than cosmetic.

**Turnstone's own tree is binary today**: `SplitChoice::First`/`Second`
([src/panes/mod.rs:272](../src/panes/mod.rs)), each branch splitting exactly
two ways. Genet's `TileTree` is **N-ary**: `children: Vec<TileBranch>`
([genet/components/genet-host-api/tile.rs:42](../../genet/components/genet-host-api/tile.rs)).
Arbitrarily nested splits are already expressible in the binary tree by
nesting — a row of three is a row of two whose second child is another row —
but N-ary children are the honest representation, and they make an even split
of three cells one node instead of a lopsided pair. So "nested splits" is
partly here and would be cleaner under `TileTree`.

**Nested panes** — a pane whose content is itself a pane tree — is what the
workbench already is (a leaf holding platen's tiling) and what a tab-stack is
(a leaf holding N tabbed tiles). Generalizing it means a leaf's content can be
another `TileTree`, which is exactly the recursion `TileTree` already has and
turnstone's binary tree does not express uniformly.

**Floating panes** are the one genuinely new structure. A floating pane is not
in the split tree at all; it is a free-rect surface composited above it, with
its own z-order. The compositor already places surfaces at rects and already
manages a chrome layer above the panes, so the machinery is present; what is
new is a second collection beside the tree — floats are siblings of the root
split, not children of any branch — and a rule for focus and z-order among
them. A torn-out pane that has not yet become its own window is the natural
first float.

The ordering these imply: adopt `TileTree` (or decide against it) in §5 first,
because nested splits and nested panes both want its recursion and its N-ary
children; floats come after, as a layer beside whichever tree wins, because
they do not depend on the tree's shape.

## 5. The Cambium boundary

The recorded ruling stands
([genet/docs/2026-07-24_frisket_pane_component_direction.md](../../genet/docs/2026-07-24_frisket_pane_component_direction.md)):
turnstone's outer renderer is a per-surface compositor, and `cambium::frisket`
does not replace it. That doc's own revisit clause is "only if turnstone ever
wants tabs", which it now plausibly does, so shared `TileTree` adoption
reopens through exactly one mixed-surface tab-stack proof.

**Fallback, stated in advance:** if the proof fails, turnstone keeps its own
tree and the ruling stands unchanged. Reversibility written down is what keeps
a failed experiment from lingering as a half-migration.

Layer responsibilities: genet-host-api owns generic topology, tab handles, and
content addresses; Cambium owns the frame, tab strip, divider, lists, grids,
settings controls, and empty/error states; Turnstone owns the registry,
bindings, persistence, and mixed-surface composition; mere domains own
portable graph vocabulary; Genet/Sprigging own DOM, style, layout, paint,
input, accessibility, and custom leaves.

Naming collision to clear during the registry work, while it is nearly free:
turnstone's field `self.frisket` is the pane tree, `cambium::frisket` is DOM
presentation, and the frisket name is already slated to move. Rename the
field.

## 6. Settings

Ownership stays as designed: the product owns typed storage, providers
describe settings, Cambium renders controls, the host applies them. The
current implementation is incomplete, verified:

- the reference is `pelt/appearance`, not a Turnstone namespace
  ([src/settings_provider.rs:16](../src/settings_provider.rs));
- rendering matches exact ids like `theme.id` instead of interpreting
  `SettingControl` ([src/settings_pane.rs](../src/settings_pane.rs));
- theme and zoom are marked `Live` but saving reaches only the provider,
  never the running shell;
- theme id and mode are free text where registered choices belong;
- scope and movement render as Rust debug values.

Settings becomes `PaneBinding::Settings(SettingsRef)`. Presentation is
user-configurable (tab, dedicated pane, modal), consistent with the
configurability-over-defaults posture. Settings is deliberately **not** the
registry's proof case; it is the special case. Publishing, a plain workflow
pane, is the proof.

## 7. The chrome

Inventory, verified. Three pieces exist: the omnibar (an overlay line with a
`>` command lane, suggestions recomputed per keystroke,
[src/app/omnibar_arms.rs](../src/app/omnibar_arms.rs)); the action catalog
(one composition read by the `>` lane, the snapshot, and the automation
runner alike, contextual rows leading, denizen rows already extending it,
[src/app/palette.rs](../src/app/palette.rs)); and the focus model, where
`FocusTarget { Canvas, Chrome, Content }` makes chrome a first-class layer.
Configurability today: content yes (the catalog is data-driven and
gate-extensible), composition no (which chrome exists, and where, is
hard-coded).

Direction, settled 2026-08-08: the zellij posture. Chrome is made of the same
stuff as content, and a named layout declares its chrome the way it declares
its panes. Four extensions, each landing on something already built:

- **Scrollback.** The typed `AppEvent` journal
  ([src/observe.rs](../src/observe.rs)) already records commands and outcomes,
  attributed. A scrollback is a chrome-facing lens over it, a dialogue view,
  not a new log. The doctrine pair is already written in the tree: "the graph
  is the history made spatial"; scrollback is the same history made temporal.
  Boundary: Steward owns live operational status, scrollback owns the
  conversation. Same events, two projections.
- **Floating panes.** Every pane already composites as its own surface, so a
  floating pane is a `PaneRecord` whose rect is its own rather than derived
  from the tree: a floating set beside the tree, per space. This extends the
  recorded tear-out trichotomy with a fourth station (tile, stack, float,
  window), recorded here as a doctrine amend

Rerun blueprints (recorded truth separate from saved view topology and
per-view config), egui_tiles (layout tree generic over app-owned payloads),
Zellij (declarative, restorable named layouts). For turnstone: saved layouts
independent of graph and session truth, with Browse/Inspect/Operate as
editable defaults rather than fixed modes.

## 8. Steps

1. **Boundary inventory.** Every pane's owner, binding, uniqueness, renderer,
   state, capabilities, evidence. Canvas/Orrery is inventoried as
   `binding: Graph(id)`, not as a "primary surface" category.
   Done when nothing relies on implicit scope.
2. **Pane registry.** Retire `PaneKind`, the mapping, the magic strings, the
   `__placeholder__`, and `System`; adopt the no-legacy-friction persistence
   posture; rename the `frisket` field; record the active-graph derivation
   rule beside the registry.
   Done when Publishing needs one registration plus its renderer.
3. **Instance correctness.** Key multi-instance renderer state by `PaneId`,
   with eviction: closing a pane drops its runner, reload rebuilds lazily.
   The runners are `!Send` and retained, so instance keying without eviction
   is a leak the singleton model could not have.
   Done when a window splits across two graph panes with independent cameras,
   selection, and physics, and tool panes follow focus between them. The
   two-graph split is the receipt because it is the hardest case; two Rosters
   fall out of the same fix.
4. **Stacks, tabs, and the tree decision.** One real tool stack over mixed
   surface types, then the shared-`TileTree` decision (§5), with the recorded
   fallback. This decision now carries more than tabs: turnstone's tree is
   binary, `TileTree` is N-ary and recursive, and nested splits plus nested
   panes both want that recursion (§4b). Weigh it as the tree question, not
   the tab question.
   Done when inactive tabs neither render nor receive input and the stack
   survives save, reload, and tear-out.

4c. **Omnibar scrollback and configurable chrome.** Surface the `AppEvent`
   journal as a bounded, per-space, act-on-again scrollback (§4b); make chrome
   presence and placement settings that §6's lane interprets. Independent of
   the tree decision, so it can proceed in parallel with step 4.
   Done when a command run in the omnibar leaves a scrollback entry whose
   result can be re-invoked, and chrome layout persists in a named layout.

4d. **Floating panes.** A free-rect surface layer beside the split tree, with
   z-order and focus rules; a torn-out-but-not-yet-windowed pane as the first
   float. After the tree decision, because floats layer onto whichever tree
   wins.
   Done when a pane floats above the tree, holds focus and z-order, and either
   redocks into the tree or tears out to a window.
5. **Cambium promotion.** Generic pane shell, settings form, empty/error/
   unavailable states into the component catalog.
   Done when narrow and regular specimens pass visual and semantic receipts.
6. **Settings completion.** Turnstone namespace, render from
   `SettingControl`, registered choices for theme id and mode, human-readable
   scope and movement, and every `Live` setting wired to observable runtime
   behavior.
   Done when theme and zoom visibly change, persist, and reload.
7. **Named layouts.** Topology and pane configuration stored separately from
   graph and session truth. The workbench boundary applies here: a layout
   captures that a workbench pane exists and what it is bound to; which
   members are open inside it is session truth and stays out.
   Done when a layout saves, restores, resets, and migrates without touching
   graph or session data.

## 9. Open, deliberately not decided here

- What a session means when one window views two graphs, and what Overmap
  shows for such a window. The doctrine change in section 2 forces the
  question; answering it belongs with the Overmap owner.
- Whether Gloss is Many or per-space. It carries per-instance composition, so
  two differently composed Glosses are genuinely useful; start Many and let
  use argue.
- Write-side pane extension (third-party registered panes through the
  participant gate). The registry's shape should not preclude it; nothing
  here builds it.
