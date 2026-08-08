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

## 7. Prior art

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
4. **Stacks and tabs.** One real tool stack over mixed surface types, then
   the shared-`TileTree` decision, with the recorded fallback.
   Done when inactive tabs neither render nor receive input and the stack
   survives save, reload, and tear-out.
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
