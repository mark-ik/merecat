# Pane Registry, Graph Views, and Shell Composition Plan

**Date**: 2026-08-08

**Status**: Direction revised; implementation not started.

**Scope**: Make pane instances independent, restore real multi-graph composition,
generalize the window layout, and make the shell configurable without collapsing
graph truth, Workbench arrangement, pane layout, and chrome into one model.

The short ruling:

- A window is a **space** containing panes. It is not owned by one graph.
- A graph runtime owns graph truth. A Forme runtime owns arrangement and physics.
  A **Graph pane** is a view onto both.
- A **Workbench** is a durable arrangement of opened graph members, hosted in a pane.
- A pane may publish graph, member, session, or application context for followers.
- The **shell** owns commands, navigation, focus, notifications, and transcript data.
  **Chrome** is the configurable projection of those services into a space.
- Tiled containers are recursive. Floating panes are a sibling layer above the tiled
  root. Tear-out moves the same pane instance into another space.
- Roster, Inspector, Apparatus, Trail, Alembic, Steward, Comms, Gloss, Overmap,
  Publishing, and Settings keep their separate meanings.

---

## 1. Prior decisions this plan carries forward

| Source | Durable decision | Consequence here |
| --- | --- | --- |
| [Multi-Graph Activation](../../mere/design_docs/mere_docs/implementation_strategy/2026-06-09_multi_graph_activation_plan.md) and the archived [Window Composition Plan](../../mere/design_docs/archive_docs/2026-06-19_completed_plans/2026-06-11_window_composition_plan.md) | Graph authorities are pooled by `GraphId`; panes resolve through their own graph binding; one window may contain panes from different graphs. | Turnstone restores this model instead of extending its current single `App::canvas`. |
| [Rung 5 Panes](2026-07-14_turnstone_rung5_panes_plan.md) | The window pane tree and Platen Workbench nest because they arrange different identities. | They may share a topology algebra, while pane identity and member identity remain distinct. |
| [Surfaces in Cambium](2026-07-15_turnstone_surfaces_in_cambium.md) | Cambium owns the furniture inside a pane rect; Turnstone owns rects, mixed surfaces, routing, and composition. | `cambium::frisket` does not become Turnstone's outer renderer. |
| [Gloss Composite Pane](2026-07-20_gloss_composite_pane.md) | Composition is per pane instance and travels with it. | `PaneConfig` remains instance state; the registry does not absorb it into pane-kind globals. |
| [Frisket Direction](../../genet/docs/2026-07-24_frisket_pane_component_direction.md) | `genet-host-api::TileTree` is presentation vocabulary; hosts remain authoritative. Turnstone's mixed-surface presenter deliberately differs from the DOM presenter. | Reuse of `TileTree` is proof-gated and does not imply adopting the Cambium DOM view. |
| [Overmap](../../mere/design_docs/mere_docs/implementation_strategy/2026-07-20_overmap_sessions_graph_plan.md) | Overmap derives a session graph from manifests and lineage. | A multi-graph space changes focus and composition, not Overmap's underlying truth. |
| [Configuration Ownership](../../mere/design_docs/mere_docs/implementation_strategy/2026-08-06_configuration_ownership_settings_projection_plan.md) | The configured product owns typed storage; providers describe; Cambium renders; hosts apply. | `SettingsRef` is a pane source, not a graph-binding policy or a universal settings store. |

The external donors agree on the broad shape:

- [Rerun Blueprints](https://rerun.io/docs/concepts/visualization/blueprints)
  separate recorded truth from saved view hierarchy and per-view configuration.
- [`egui_tiles`](https://docs.rs/egui_tiles/latest/egui_tiles/) uses one recursive,
  app-payload-generic tree with explicit
  [container cleanup policy](https://docs.rs/egui_tiles/latest/egui_tiles/struct.SimplificationOptions.html).
- [Zellij layouts](https://zellij.dev/documentation/creating-a-layout.html) treat
  tiled panes, stacked panes, floats, chrome choices, and named layouts as data.

They are donors, not replacement ontologies:

| Donor | Take | Keep out |
| --- | --- | --- |
| Rerun | Truth/blueprint separation; nested Grid, Horizontal, Vertical, and Tabs containers; editable, file-backed, default and heuristic layouts | Using application-id coupling as Mere's space identity; Mere spaces can deliberately compose several graphs |
| `egui_tiles` | Generic payloads, recursive containers, grid movement, drag/drop, garbage collection, and explicit simplification rules | Making a runtime UI tree durable authority before Turnstone proves serialization and mixed-surface routing |
| Zellij | Named nested layouts; tiled/stacked/floating stations; [pane identity and pin/show/hide operations](https://zellij.dev/documentation/plugin-api-commands) | Treating a terminal scrollback buffer as the semantic shell transcript; allowing floats to become a second copy of a tiled pane |

## 2. Live contradictions

### 2.1 Graph identity exists in the model and is dropped by the host

Every `PaneNode::Leaf` already carries `graph_id`, and
`retag_graph_bound_from` explicitly preserves a second graph pane
([src/panes/layout.rs](../src/panes/layout.rs)). The current host then loses the
binding:

- `App` owns one `canvas` and one current `session_id`
  ([src/app/mod.rs](../src/app/mod.rs)).
- Session adoption replaces the canvas graph in place
  ([src/app/session_lifecycle.rs](../src/app/session_lifecycle.rs)).
- `PanePlacement` carries pane id, content, and rect, but not `graph_id`
  ([src/pane.rs](../src/pane.rs)).
- Every Orrery leaf becomes the same `SurfaceKind::Canvas`, with one fixed
  `SurfaceId::CANVAS` ([src/surface.rs](../src/surface.rs)).
- A lens borrows the same canvas and temporarily swaps only its viewport
  ([src/shell/lens.rs](../src/shell/lens.rs)).

Two Orrery leaves can therefore duplicate one canvas view, but cannot present two
independent graphs. The old Mere pool is precedent, not current Turnstone proof.

### 2.2 Pane identity exists and renderer state is keyed by kind

`summon_pane` mints a fresh `PaneId` on every summon
([src/app/pane_arms.rs](../src/app/pane_arms.rs)), while the shell retains one
Roster, Gloss, Trail, Inspector, Workbench, Apparatus, Settings, and Publishing
runner ([src/shell/mod.rs](../src/shell/mod.rs)). Sharing one runner across a pane's
tear-out stations preserves identity. Sharing it across two distinct pane ids
does not.

### 2.3 Pane registration is distributed

`PaneKind`, `PaneContent`, `pane_content`, labels, palette rows, render arms, input
arms, observation, and accessibility all repeat pane-kind knowledge. Settings and
Publishing use `Custom("settings")` and `Custom("publishing")`; layout mutation uses
`Custom("__placeholder__")`; `System` has no live constructor.

### 2.4 The Workbench and chrome are app-global

Turnstone holds one `App.workbench`, so two Workbench panes cannot own different
member arrangements. The omnibar likewise queries one `App::canvas`; its width and
top offset are constants, and only the primary window renders it
([src/ui.rs](../src/ui.rs), [src/chrome_view.rs](../src/chrome_view.rs)). Application
settings already name `shellbar_edge` and `shellbar_hidden`, but Turnstone does not
apply them.

### 2.5 `AppEvent` is not shell scrollback

`AppEvent` is a useful semantic observation stream, but it is partial, drained each
frame, and frequently carries display text without command identity, target context,
correlation, or a typed result ([src/observe.rs](../src/observe.rs)). A visible shell
transcript can consume those events; it cannot truthfully be a direct view over the
current stream.

## 3. Separate source, context, and view state

The earlier model overloaded `PaneBinding` with three different questions. The
replacement keeps them distinct:

```rust
struct PaneSpec {
    id: PaneId,
    kind: PaneKindId,
    source: PaneSource,          // what this pane presents
    context: ContextBinding,     // what this pane follows
    config: PaneConfig,          // durable per-instance configuration
}

struct PaneRecord {
    spec: PaneSpec,
    view: PaneViewState,         // pane-local camera, selection, scroll, UI state
}

enum PaneSource {
    Fixed(SourceRef),
    FromContext(SourceSelector),
}

enum SourceRef {
    Graph(GraphId),
    Forme { graph: GraphId, forme: FormeId },
    Member { graph: GraphId, member: GraphMemberId },
    Settings(SettingsRef),
    Session(SessionId),
    SessionSet,
    Application,
    External {
        schema: SourceSchemaId,
        payload: SerializedSource,
    },
}

enum SourceSelector {
    Graph,
    Forme,
    Member,
    Session,
}

enum ContextBinding {
    Own,
    Follow(PaneId),
    FocusedInOwnSpace,
    Application,
}
```

The exact Rust shapes may narrow during A0. The separation is the requirement.

### Context publication

A focused pane publishes the context it can honestly supply:

```rust
struct PaneContext {
    graph: Option<GraphId>,
    forme: Option<FormeId>,
    member: Option<GraphMemberId>,
    session: Option<SessionId>,
}
```

- A Graph pane publishes its graph and selected member.
- A Workbench publishes its graph and active member.
- A pinned member pane publishes its graph and member.
- Overmap publishes the selected session.
- Application surfaces publish no graph context.

`FocusedInOwnSpace` resolves to the most recently focused pane in the pane's current
space that publishes the needed field. Moving or tearing out the follower therefore
changes its focus scope without rewriting a stored `SpaceId`. `Follow(pane)` remains
attached to one context source. A following Roster, for example, uses
`PaneSource::FromContext(SourceSelector::Graph)` with `FocusedInOwnSpace`; pinning it
resolves that pair to `PaneSource::Fixed(SourceRef::Graph(id))` with `Own`.

The omnibar captures a context snapshot when it opens. A later focus movement cannot
silently redirect a command that is already being composed.

### Graph authority and Graph pane

Graph truth and projected arrangement are runtimes, not panes:

```rust
GraphRuntimePool: HashMap<GraphId, GraphRuntime>
FormeRuntimePool: HashMap<(GraphId, FormeId), FormeRuntime>
```

`GraphRuntime` owns graph truth and graph-scoped live resources. `FormeRuntime` owns
arrangement geometry and its physics, with shared geometry addressed inside it by
projection kind and layout id. A Graph pane owns a `PaneId`, camera, selection, and
other view intent. Two panes may show:

- two different graphs, using two runtimes;
- one graph through independent cameras and selections;
- one graph through a shared `FormeId` geometry;
- explicit arrangement forks, using different `FormeId` values.

The identity Orrery arrangement is computed from graph truth. It is not persisted as
if it were a curated Workbench. Curated member arrangements and explicit Forme forks
are durable; projection geometry may be rebuilt from them.

`SurfaceKind::Graph(PaneId)` replaces the singleton canvas surface. Render, input,
accessibility, content routing, and commands resolve the pane first, then its graph.

### Session rule

A session remains a durable graph-shaped unit with `SessionId -> root GraphId`.
A space may reference several sessions and graph runtimes at once. Focus chooses the
default target for switch, save, rename, and navigation actions. Overmap continues to
derive the session set from manifests. Broader cross-session relations stay with the
Overmap and murm/moot owners.

This minimum rule is owned here because a two-graph receipt cannot be implemented
while `App::session_id` still means the only graph allowed in memory.

## 4. The Workbench

The Workbench is a graph-bound desk: a durable, curated arrangement of graph members
promoted into directly usable content surfaces. It answers which members are open,
how they are split or stacked, and which member is active. It does not replace graph
truth or the window layout.

Platen already owns a recursive split tree of member tab-stacks and projects it onto
`genet-host-api::TileTree`
([Platen README](../../mere/crates/platen/platen/README.md),
[workbench.rs](../../mere/crates/platen/platen/src/workbench.rs)). Turnstone currently
hosts that tree inside one Workbench pane and composites live document surfaces into
its cells ([src/workbench_pane.rs](../src/workbench_pane.rs)).

The durable relationship is:

```text
GraphId
  -> FormeId / Workbench arrangement
       -> Workbench pane view
            -> member tiles and document surfaces
```

A window blueprint stores the Workbench pane and its
`PaneSource::Fixed(SourceRef::Forme { graph, forme })` source. The member arrangement
stays with the Forme/Workbench store. Closing its last pane makes the arrangement
dormant; reopening it restores the same member tree.

### Shared topology, separate authority

The useful generalization is a topology algebra parameterized by leaf identity:

```text
window composition  = LayoutTree<PaneId>
workbench content   = LayoutTree<MemberRef>
```

They may share split, tab, grid, drag, fraction, and normalization code. They do not
share ids, registries, or persistence owners. A Workbench member tile is pane-shaped
and may render as its own surface, but it is not a registered window pane.

The current hard sentence "Workbench cells are not panes" is therefore refined:
they are nested layout leaves keyed by graph-member identity rather than top-level
pane identity.

## 5. Space topology, nesting, and floats

```rust
struct SpaceBlueprint {
    panes: Vec<PaneSpec>,
    tiled: LayoutNode,
    floating: Vec<FloatingPane>,
    chrome: ChromeBlueprint,
}

enum LayoutNode {
    Pane(PaneId),
    Split {
        axis: Axis,
        children: Vec<LayoutBranch>,
    },
    Tabs {
        children: Vec<LayoutNode>,
        active: usize,
    },
    Grid {
        children: Vec<LayoutNode>,
        columns: GridColumns,
        shares: GridShares,
    },
}
```

Containers recurse, so nested splits and tabbed sublayouts are ordinary structure.
The model must define normalization after every edit: empty containers disappear,
single-child containers collapse according to policy, adjacent same-axis splits may
join, fractions renormalize, and focus moves to a surviving neighbor. The policy is
saved with or derived from the named layout, not scattered through gesture handlers.

### Nested panes

Three cases must keep different names:

1. A recursively nested **container** holds ordinary pane leaves.
2. A **layout-host pane**, currently Workbench, renders a second tree whose leaves use
   another identity type.
3. A pane containing another arbitrary `SpaceBlueprint` is a future embedded space.
   It waits for a real second consumer beyond Workbench.

### Floating panes

A float is the same `PaneId` presented outside the tiled tree:

```rust
struct FloatingPane {
    pane: PaneId,
    rect: RelativeRect,
    z: u32,
    pinned: bool,
    visible: bool,
}
```

Each `PaneId` occupies exactly one station across all live spaces: one tiled leaf or
one floating entry. Floating is a change of station, not a second appearance.

Docking, floating, and tearing out relocate one pane instance. They do not clone its
runtime. Floats are scoped to a space, composite above the tiled root, and below
modal chrome. Clicking raises a float. A pinned float stays visible when the float
layer is toggled. Geometry supports proportional and absolute constraints so a
layout survives window resizing.

### The `TileTree` decision

The existing `genet-host-api::TileTree` is more capable than Turnstone's binary
split tree in some ways, but it is not the complete target above:

- it has N-ary row/column splits and leaf tab-stacks;
- tab-stacks contain tiles, not arbitrary child subtrees;
- it has no grid or float layer;
- its current types are runtime presentation values rather than serde persistence;
- its own module contract says Mere projects Forme onto it rather than making it
  arrangement truth.

A0 therefore defines the required topology before choosing the reused type. A4 must
prove one of these outcomes:

1. Extend the shared contract because Pelt and Turnstone both consume the added
   topology without importing Mere semantics; or
2. Keep Turnstone's serializable `SpaceBlueprint` authoritative and project it onto
   `TileTree` where Cambium furniture benefits; or
3. Keep the Turnstone presenter and tree independent while sharing only split/tab
   state math.

The mixed-surface compositor remains Turnstone-owned in every outcome.

## 6. Pane registry and taxonomy

```rust
struct PaneDefinition {
    id: PaneKindId,
    display_name: &'static str,
    source_shape: SourceShape,
    uniqueness: Uniqueness,
    capabilities: PaneCapabilities,
    default_placement: PlacementPolicy,
    config_schema: PaneConfigSchema,
    config_codec: PaneConfigCodec,
    view_persistence: ViewPersistencePolicy,
    renderer_factory: RendererFactory,
}

enum Uniqueness {
    Many,
    PerSpace,
    PerSpaceAndSource,
    PerSpaceAndContext,
}
```

Each definition owns the typed config schema, default, and encode/decode boundary
for its pane kind. `PaneConfig` is not a universal untyped property bag.
`PaneSource` names content authority rather than repeating pane kind; the
definition's `source_shape` validates which source forms that kind accepts.

Initial rulings:

| Pane | Plain job | Source/context | Uniqueness |
| --- | --- | --- | --- |
| Graph / Orrery | Spatial graph view and context source | fixed Forme | Many |
| Workbench | Curated member workspace and context source | fixed Forme | Many |
| Tile | One pinned member surface | fixed member | Many |
| Gloss | Graph navigator and projection | graph context; per-instance composition | Many |
| Roster | Graph manifest | graph context | Per space and context |
| Inspector | Addressed-content identity and diagnostics | member context | Per space and context |
| Apparatus | Selected object's facets and representation | member context | Per space and context |
| Trail | Navigation chronology | graph/session context | Per space and context |
| Alembic | Durable memory and engrams | application/persona source | Per space and source |
| Steward | Operational activity | application/space source | Per space |
| Comms | Conversation | place/session source | Per space and source |
| Overmap | Session set and lineage | session-set source | Per space |
| Publishing | Publishing workflow | explicit target source | Many |
| Settings | Provider-described configuration | `Settings(SettingsRef)` | Per space and source |

`System` is removed. `Custom(String)` becomes a namespaced `External` source whose
schema owns decoding and validation. The layout placeholder becomes an internal
`LayoutNode` edit state rather than a fake pane. `PaneKind` retires; the registry
supplies labels, palette entries, availability, capabilities, uniqueness, and
renderer creation.

Renderer state is keyed by `PaneId`. Tear-out preserves the id. Closing the pane
evicts the retained runner; restore rebuilds it lazily.

## 7. Shell services, chrome, and scrollback

Chrome is not the pane tree, its dividers, or graph content. It is the visual
projection of shell services above or beside the space:

- omnibar and command providers;
- focus and command targeting;
- navigation controls;
- notifications and transient status;
- layout editing affordances;
- shell transcript projections.

The shell owns the data and actions. `ChromeBlueprint` decides which projections
exist and where they appear.

```rust
struct ChromeBlueprint {
    omnibar: OmnibarPlacement,
    shellbar: Option<ShellbarLayout>,
    transcript: TranscriptPlacement,
    status: StatusPlacement,
}
```

Placements may be summoned overlay, docked edge, floating pane, ordinary pane, or
hidden. Application settings choose defaults and shortcuts. A named space blueprint
stores the actual composition. Per-space transient state such as open/closed, current
query, selection, and scroll offset stays outside the reusable blueprint.

### Shell transcript

The transcript is a typed, bounded record of intentional shell interaction, rather
than a tracing console:

```rust
struct ShellEntry {
    id: ShellEntryId,
    input: ShellInput,
    resolved_intent: Option<ShellIntent>,
    target: ContextSnapshot,
    outcome: ShellOutcome,
    timestamp: Timestamp,
    privacy: EntryPrivacy,
}
```

`AppEvent` and asynchronous updates enrich entries through correlation ids. They are
also usable independently by diagnostics and automation. Steward keeps operational
status; Comms keeps conversation; the Shell Transcript keeps the user's command,
navigation, and result history.

The transcript can project as recent omnibar history, a docked Command Log, a
floating pane, or an ordinary pane. Entries may be copied, repeated, or opened at
their original target. Retention and persistence are configurable. Storage is local
by default; providers redact secret-bearing input before it reaches the ledger.

## 8. Settings

Settings remains addressed configuration content:

- `PaneSource::Fixed(SourceRef::Settings(settings_ref))` selects the provider and
  page;
- `ContextBinding` states whether the page follows a context or remains application
  scoped;
- the configured product owns typed values and storage;
- `SettingsProvider` describes and applies them;
- Cambium renders controls from `SettingControl` rather than setting ids.

Turnstone's current provider still uses `pelt/appearance`, exposes only theme id,
theme mode, and zoom, and marks them `Live` without updating the running shell
([src/settings_provider.rs](../src/settings_provider.rs),
[src/settings_pane.rs](../src/settings_pane.rs)). The first completion slice corrects
the namespace and makes every advertised `Live` value observable immediately.

Apparatus remains object-facing. Diagnostics remain outside settings. Presentation
as tab, pane, float, or modal is a chrome/layout preference over one Settings source.

## 9. Persistence ownership

| State | Owner and storage |
| --- | --- |
| Graph truth and graph-scoped runtime metadata | Session/graph stores keyed by `GraphId` |
| Forme identity, lifecycle, and curated Workbench member arrangement | Durable Forme/Workbench store keyed by `(GraphId, FormeId)` |
| Identity Orrery arrangement | Computed from graph truth rather than copied into a window blueprint |
| Shared projection geometry and physics | `FormeRuntime`, addressed by `(FormeId, projection kind, layout id)` and rebuilt from graph/Forme truth |
| Graph-pane camera, selection, and view intent | Live `PaneRecord` keyed by `PaneId`; the pane definition decides which fields snapshot into a blueprint |
| Pane source, follower rule, and instance configuration | `PaneSpec` in `SpaceBlueprint` |
| Tiled topology, floats, and chrome composition | `SpaceBlueprint` |
| Application shell defaults and settings | Application settings provider |
| Shell transcript | Bounded local transcript store under explicit retention policy |

Rerun's lesson applies directly: truth and blueprint are independently useful. A
layout can be saved, shared, reset, or heuristically regenerated without editing a
graph or Workbench arrangement. A Workbench source in a portable blueprint may name
a role such as "focused graph's default workbench" rather than exporting a private
local `FormeId`.

The repository is pre-alpha. The first registry/layout format may replace existing
`frame.json`; unreadable legacy layouts fall back to a logged default. Graph,
Workbench, settings, and transcript stores are not discarded with it.

## 10. Implementation gates

### A0. Pure model and inventory

- Inventory every current pane's source, published context, follower rule,
  uniqueness, instance state, capabilities, renderer, persistence, and evidence.
- Define `PaneSpec`, `PaneRecord`, `PaneSource`, `ContextBinding`, `PaneContext`,
  `SpaceBlueprint`, normalization, and focus resolution as data-only types.
- Add model tests for nested containers, focus-derived context, pinning, float moves,
  cross-space PaneId uniqueness, normalization, and serde round trips.

Done when graph/source/context/multiplicity are explicit and no rendering code is
needed to prove the state transitions.

### A1. Registry and instance correctness

- Register existing panes and retire `PaneKind`, `pane_content`, `System`, magic
  custom strings, and the fake placeholder.
- Use Publishing as the plain workflow proof.
- Key retained runners by `PaneId`; preserve ids across docking and tear-out; evict
  on close and rebuild lazily on restore.

Done when adding a Publishing-like pane requires one registration plus its renderer,
and two supported same-kind panes retain independent scroll, selection, and controls.

### A2. Graph runtime pool and two-graph composition

- Replace `App::canvas` and exclusive `session_id` routing with a graph runtime pool
  plus Forme runtimes and focused-space context.
- Carry `PaneId` and `graph_id` through placement, surface planning, render, input,
  content contributions, accessibility, commands, save, and observation.
- Replace singleton Canvas focus/surface identity with pane-addressed Graph surfaces.

Done when graph A and graph B render side by side in one window, both receive pointer
and keyboard input, graph A mutation leaves B unchanged, and restart restores both.

### A3. Multiple views and Workbenches

- Allow two Graph panes over one graph with independent cameras and selections.
- Create or reuse the `FormeRuntime` named by each Graph and Workbench pane's Forme
  source.
- Share geometry only when Forme, projection kind, and layout id agree; keep camera,
  selection, and other view intent keyed by `PaneId`.
- Key Workbench state by graph/Forme source instead of `App.workbench`.
- Publish context from Graph, Workbench, and member panes; make Roster and Inspector
  follow explicit or focused context.

Done when one graph appears through two independent views, two Workbenches can show
different arrangements, and Inspector follows the active member of the intended
Workbench rather than a global canvas.

### A4. Containers, tabs, and shared-tree decision

- Prove nested N-ary splits and a tabbed subtree over mixed Graph, document, and
  Cambium surfaces.
- Compare the required `SpaceBlueprint` operations with `TileTree` and record outcome
  1, 2, or 3 from section 5.
- Keep inactive tabs out of render, hit testing, accessibility focus, and pumping
  unless their content lifecycle explicitly remains warm.

Done when a nested mixed-surface layout survives drag, resize, save, reload, and
tear-out, with one authoritative topology and the fallback recorded.

### A5. Floating layer

- Add float geometry, z-order, pinned visibility, focus raise, dock targets, and
  tear-out from a float.
- Reuse the same PaneId and renderer state through every station.

Done when one pane moves tile -> float -> nested split -> window and returns without
losing state or leaving an unreachable runner.

### A6. Shell and configurable chrome

- Split shell service state from Chrome projections.
- Add provider registration and configurable omnibar placement, row limit, default
  scope, shortcuts, shellbar edge/visibility, and transcript placement.
- Add `ShellTranscript` with correlation, privacy, retention, and repeat/open actions.

Done when a command captures its original pane context, produces one correlated
result entry, can be repeated from a docked or floating transcript, and the chrome
composition restores from a named layout.

### A7. Cambium and Settings completion

- Promote pane shell/header, settings form, empty/error/unavailable state, and the
  relevant Frisket specimens into Cambium's component catalog.
- Correct the Turnstone Settings namespace and render generically from
  `SettingControl`.
- Connect every `Live` setting to the running host; label restart/startup settings
  honestly.

Done when catalog specimens cover narrow and regular layouts, Turnstone consumes
them, theme/zoom/shell placement change live, and values survive relaunch.

### A8. Named blueprints

- Save, load, duplicate, rename, reset-to-default, and reset-to-heuristic layouts.
- Keep private ids out of portable exports; resolve role-based sources on import.
- Preserve graph, Workbench, Settings, and transcript stores across layout changes.

Done when Browse, Inspect, and Operate are editable examples rather than modes, and a
layout can be shared or reset without changing graph/session truth.

## 11. Stop rules

- Keep Turnstone's mixed-surface compositor as the outer presenter.
- Treat graph authority, Graph pane view state, Workbench arrangement, and window
  blueprint as separate owners even when they share tree or surface primitives.
- Keep PaneId stable across dock, float, and tear-out; use member identity inside a
  Workbench.
- Require a second consumer before promoting arbitrary embedded-space hosting or new
  shared topology into Genet.
- Keep `AppEvent` useful for observation; create a correlated transcript rather than
  changing it into a UI store.
- Keep generated proof outputs on disk and outside Git.
- Stop a shared-`TileTree` migration if it cannot preserve mixed-surface rendering,
  persistence, instance identity, and input authority in one completed proof.

## 12. Deferred decisions

- Write-side third-party pane registration through the participant gate.
- Stored cross-session Overmap relations beyond manifest-derived lineage.
- Product display-name changes such as Canvas versus Orrery and Navigator versus
  Gloss.
- Default layout details. Defaults remain editable and user-configurable.
