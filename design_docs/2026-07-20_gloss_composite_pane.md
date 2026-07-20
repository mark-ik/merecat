# Gloss as a composite pane; the swatch as a customizable projection

2026-07-20, Mark's ask (during the recycle-bin work): "at base it's a
minimap, but if you wanted to you could put sections from other panes in
there too (like recent nodes, or deleted)" — and the swatch itself "can and
should be customizable... must consider the UI for this." Ruled POSSIBLE;
this note records the shape so the build starts from it. Design capture
only; nothing here is implemented.

## Why it is structurally cheap

Both halves already exist as the right kind of pieces:

- **Sections are already the currency.** Trail is a `sectioned_list` over
  neutral `ListSection`s, and every pane's content is gathered by a pure
  `fn(&App) -> rows` (trail_rows, the roster gather, the recycle bin's
  removed-derive). Naming these — a **section-provider registry**, id →
  gather — makes any list section composable into any pane. The providers
  are the functions we already have.
- **The swatch is already a projection.** The Gloss minimap is a sprigging
  custom-paint leaf over a cartography projection of the graph. "Customizable
  swatch" = the projection's parameters (color-by, filter, emphasis, layout
  source) become a named preset the pane config carries — the register-lens /
  cartography vocabulary, not new machinery.

## The shape

- `GlossConfig { sections: Vec<SectionId>, swatch: ProjectionPreset }` —
  ordered, the swatch being section zero by default.
- **Config lives ON THE FRISKET LEAF** (arrangement truth): it persists with
  `frame.json`, tears out with the pane, differs per lens window, and needs
  no sidecar. Precedent: `PaneContent::Tile(uuid)` already carries data.
  Likely `PaneContent::Gloss(GlossConfig)` with serde defaults so old
  layouts load.
- Composition generalizes: if it proves out on Gloss, ANY list pane can host
  foreign sections (Trail's Removed inside Gloss, a roster bucket beside the
  minimap). Gloss is the pilot because its identity ("the navigator") is
  additive, not replaced.

## The UI question (the real work, per Mark)

Candidates, not yet chosen:

1. **Right-click the pane → config actions in the palette** ("Gloss: add
   section — Recent", "Gloss: remove section — Removed"). Rides the
   just-landed right-click palette; zero new chrome. Weakness: invisible
   affordance.
2. **An edit mode on the pane** (a small ⚙ row when the pane is active):
   in-place add/remove/reorder rows. Needs the pane-header furniture panes
   deliberately don't have yet.
3. **Drag a section header from one pane into another** (the tile-drag
   grammar applied to sections). The most direct manipulation, the most
   gesture work; fits the tear-out family long-term.

Recommendation when built: start with (1) — it is spine-honest, receipt-
drivable, and teaches the palette scoping we already want; grow (3) once
section headers are draggable objects. NOT Apparatus: pane composition is
arrangement/view state, not graph-object metadata (the taxonomy holds).

## Prerequisites

- The section-provider registry (extract the existing gathers behind ids).
- `PaneContent::Gloss` grows its config (serde-defaulted).
- Palette actions scoped to the active pane (the registry's category reread
  the harvest doc already earmarks).
