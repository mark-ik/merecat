//! # frisket
//!
//! The pane model: a savable tree of resizable panes, projected into a uxtree
//! subtree. On a hand press the *frisket* is the hinged frame whose cut-out
//! apertures decide what prints where; this is the same frame, over a window.
//!
//! Renamed from `frame` (2026-07-14), which was overloaded three ways in this
//! family (a rendered frame, a `TileFrame`, a window's pane arrangement) and which
//! also fused in the workspace id vocabulary. The ids now live in [`incipit`];
//! this crate is the panes alone.
//!
//! The tiers, which nest rather than compete:
//!
//! - `frisket` is pane-ness: kinds, ids, the split tree, the persisted layout.
//! - `platen` tiles graph nodes *inside* one [`PaneContent::Workbench`] leaf.
//! - The host turns this ratio tree into rects. Frisket emits no geometry.
//!
//! Split across submodules to keep each file under the workspace's
//! 600-LOC ceiling: [`layout`] holds the [`FrisketLayout`] operations
//! ([`FrisketLayout::summon_leaf`], `reparent_leaf`, `close_leaf`, …);
//! [`projection`] holds [`project_frisket`] + [`project_frisket_with`].
//!
//! [`incipit`]: https://docs.rs/incipit
//! [`platen`]: https://docs.rs/platen

#![doc(html_root_url = "https://docs.rs/frisket/0.0.1")]

use serde::{Deserialize, Serialize};

mod layout;
mod projection;

/// The frame-sidecar persistence (`frame.json` beside `graph.json`), moved
/// here from `session_runtime::frisket_store` at meerkat's deletion. Native
/// only (the `fs` path), so the crate stays wasm-clean.
#[cfg(not(target_arch = "wasm32"))]
pub mod store;

/// Tear-out gesture payload types (leaf/branch/fork), moved here with the
/// pane model — they name [`PaneId`], so they are pane vocabulary. The
/// gestures themselves land over the forest dom.
pub mod tearout;

#[cfg(test)]
mod tests;

pub use projection::{project_frisket, project_frisket_with};

/// The workspace identity vocabulary this crate binds panes to. Re-exported for
/// convenience: a pane leaf carries a [`GraphId`], and hosts that hold a
/// [`FrisketLayout`] invariably hold graph ids beside it.
pub use incipit::{GraphId, SessionId};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Lifecycle stage marker.
pub const STAGE: &str = "pre-alpha";

/// Stable identifier for a saved pane layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrisketId(pub String);

impl FrisketId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier for an individual pane within a layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

/// Direction of a split between two child panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitAxis {
    /// Children are arranged side-by-side; ratio applies to width.
    Horizontal,
    /// Children are stacked vertically; ratio applies to height.
    Vertical,
}

/// What a leaf pane shows. Extension point: `Custom` carries a
/// host-defined content kind for content not yet promoted to a
/// dedicated mere-domain module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaneContent {
    Workbench,
    Orrery,
    Gloss,
    /// The graph's manifest — every primitive (nodes / facets, edges, fields),
    /// examinable. The data-view counterpart to the orrery's space-view (see the
    /// graph-roster + frame-taxonomy design doc).
    Roster,
    /// Selected object details: provenance, trust, parse diagnostics, document
    /// structure, cache state, and lineage for the active graph object.
    Inspector,
    /// Navigation trail + memory: the focused node's url history, the graph-wide
    /// recently-visited nodes, and the eidetic deleted-nodes log.
    Trail,
    /// Live async operations: fetch/sync/content actors, retries, background work,
    /// and user-facing controls over running jobs.
    Steward,
    /// Misfin / murm messaging (the `comms` domain).
    Comms,
    /// Memory: short-term (Recent), long-term (Saved), and distilled graph
    /// engrams. The Alembic pane (memory + engrams architecture); window-chrome
    /// (its default scope is all memory, not the active graph).
    Alembic,
    Apparatus,
    /// The session set as a graph — container nodes with fork lineage; the
    /// switcher's graph view (overmap O1). Window-chrome: it is ABOUT the
    /// session set, not any one graph.
    Overmap,
    System,
    /// **Pinned tile** — a single specific node's tile rendered without a
    /// workbench strip. Per the pane-UX brief §3 frametree side-by-side
    /// rendering. Carries the node's stable id (the graph member uuid —
    /// petgraph indices shift across removals, so an index here would go
    /// stale in a persisted space); the leaf's `graph_id` scopes it. The
    /// host renders the node's live document session when one is up, else
    /// an honest placeholder.
    Tile(uuid::Uuid),
    Custom(String),
}

impl PaneContent {
    /// Compact tag suitable for tracing fields and accessible names.
    pub fn tag(&self) -> &str {
        match self {
            PaneContent::Workbench => "workbench",
            PaneContent::Orrery => "orrery",
            PaneContent::Gloss => "gloss",
            PaneContent::Roster => "roster",
            PaneContent::Inspector => "inspector",
            PaneContent::Trail => "trail",
            PaneContent::Steward => "steward",
            PaneContent::Comms => "comms",
            PaneContent::Alembic => "alembic",
            PaneContent::Apparatus => "apparatus",
            PaneContent::Overmap => "overmap",
            PaneContent::System => "system",
            PaneContent::Tile(_) => "tile",
            PaneContent::Custom(s) => s.as_str(),
        }
    }

    /// Whether this pane renders the window's **active graph** (and so re-sources
    /// when the active graph changes), versus window-chrome that is graph-independent.
    ///
    /// Graph-bound panes show the graph or its objects — the orrery's space-view,
    /// the roster's data-view, the gloss minimap, a node Inspector, the workbench's
    /// node tiles. Window-chrome panes are about the *window / system*, not any one
    /// graph: the Steward's running jobs, Comms messaging, the Apparatus / System
    /// diagnostics. On a multi-graph switch the host re-points graph-bound leaves to
    /// the new active graph (see [`FrisketLayout::retag_graph_bound`]) and leaves
    /// window-chrome untouched. (Multi-graph MG5; the model-B re-sourcing policy.)
    pub fn follows_active_graph(&self) -> bool {
        match self {
            PaneContent::Orrery
            | PaneContent::Workbench
            | PaneContent::Gloss
            | PaneContent::Roster
            | PaneContent::Inspector
            | PaneContent::Trail
            | PaneContent::Tile(_) => true,
            PaneContent::Steward
            | PaneContent::Comms
            | PaneContent::Alembic
            | PaneContent::Apparatus
            | PaneContent::Overmap
            | PaneContent::System
            | PaneContent::Custom(_) => false,
        }
    }
}

/// One node in the layout tree: either a split (two children at a
/// given axis + ratio) or a leaf (one pane showing a content kind
/// bound to a graph).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PaneNode {
    Split {
        axis: SplitAxis,
        /// Fraction of the parent occupied by `first`; `second` takes
        /// `1.0 - ratio`. Clamped by consumers to a sane minimum.
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
    Leaf {
        pane_id: PaneId,
        content: PaneContent,
        /// Which graph this panel renders. The host resolves the
        /// `GraphId` against its `GraphRegistry` to get the live
        /// `Entity<Graph>`. Multiple leaves carrying the same
        /// `graph_id` share a graph; differing IDs in one frame =
        /// multi-graph window.
        ///
        /// `#[serde(default)]` allows pre-`graph_id` layouts saved
        /// to disk to deserialize — they come back as a nil UUID,
        /// which the host stamps with the window's primary graph
        /// on load.
        #[serde(default)]
        graph_id: GraphId,
    },
}

/// One step into a [`PaneNode::Split`] when walking the layout tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SplitChoice {
    First,
    Second,
}

/// Path through the layout tree to a specific split, expressed as
/// `First`/`Second` choices at each branch. Empty path = root.
pub type SplitPath = Vec<SplitChoice>;

/// A complete frame: identity, label, and the layout tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrisketLayout {
    pub id: FrisketId,
    pub label: String,
    pub root: PaneNode,
}

impl Default for FrisketLayout {
    /// A single Orrery pane bound to the nil (unbound) graph — a placeholder the
    /// host overwrites with the real content frame at startup. It exists so
    /// per-window state can hold a `FrisketLayout` by value (the host carve uses
    /// `Default` + assignment). (Multi-window plan.)
    fn default() -> Self {
        FrisketLayout {
            id: FrisketId::new("content"),
            label: "content".to_string(),
            root: PaneNode::Leaf {
                pane_id: PaneId(0),
                content: PaneContent::Orrery,
                graph_id: GraphId::nil(),
            },
        }
    }
}

/// Where to insert a new leaf relative to an existing leaf at a
/// `SplitPath`. Used by [`FrisketLayout::summon_leaf`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsertSide {
    /// New leaf goes left of the existing leaf (horizontal split).
    Left,
    /// New leaf goes right of the existing leaf.
    Right,
    /// New leaf goes above the existing leaf (vertical split).
    Above,
    /// New leaf goes below the existing leaf.
    Below,
}

impl InsertSide {
    pub(crate) fn split_axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Horizontal,
            Self::Above | Self::Below => SplitAxis::Vertical,
        }
    }

    /// True when the new leaf goes in `first` position (left / above);
    /// false when it goes in `second` (right / below).
    pub(crate) fn new_is_first(self) -> bool {
        matches!(self, Self::Left | Self::Above)
    }
}
