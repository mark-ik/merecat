//! Operations on [`super::FrisketLayout`] — split, summon, reparent,
//! close, iterate. Split out of `lib.rs` to keep the parent module
//! under the workspace's 600-LOC ceiling.

use crate::{
    FrisketLayout, GraphId, InsertSide, PaneContent, PaneId, PaneNode, SplitAxis, SplitChoice,
    SplitPath,
};

impl FrisketLayout {
    /// Find the split node at `path` and overwrite its ratio. Returns
    /// `true` if the path resolved to a `Split`. The new ratio is
    /// clamped to `[0.05, 0.95]` so panes can't fully collapse.
    pub fn set_split_ratio(&mut self, path: &[SplitChoice], new_ratio: f32) -> bool {
        let mut node = &mut self.root;
        for step in path {
            let PaneNode::Split { first, second, .. } = node else {
                return false;
            };
            node = match step {
                SplitChoice::First => first.as_mut(),
                SplitChoice::Second => second.as_mut(),
            };
        }
        if let PaneNode::Split { ratio, .. } = node {
            *ratio = new_ratio.clamp(0.05, 0.95);
            true
        } else {
            false
        }
    }

    /// Read-only lookup of a split's `(axis, ratio)` at `path`.
    pub fn split_at(&self, path: &[SplitChoice]) -> Option<(SplitAxis, f32)> {
        let mut node = &self.root;
        for step in path {
            let PaneNode::Split { first, second, .. } = node else {
                return None;
            };
            node = match step {
                SplitChoice::First => first.as_ref(),
                SplitChoice::Second => second.as_ref(),
            };
        }
        if let PaneNode::Split { axis, ratio, .. } = node {
            Some((*axis, *ratio))
        } else {
            None
        }
    }

    /// Summon a new leaf adjacent to an existing leaf. Walks `path`
    /// to the target leaf, replaces it with a split whose two
    /// children are the original leaf and the new one (in the order
    /// dictated by `side`). The new split inherits a default 50/50
    /// ratio.
    ///
    /// Returns `true` when the path resolved to a leaf; `false`
    /// otherwise (the layout is unchanged). Panel-summon buttons in
    /// the shellbar call this with `path = []` (anchor at root)
    /// and `side = InsertSide::Right` to add panels along the
    /// right edge by default.
    pub fn summon_leaf(
        &mut self,
        path: &[SplitChoice],
        side: InsertSide,
        new_leaf: PaneNode,
    ) -> bool {
        debug_assert!(matches!(new_leaf, PaneNode::Leaf { .. }));
        // Walk to the parent of the target so we can swap the child in place.
        let target = walk_mut(&mut self.root, path);
        let Some(target) = target else {
            return false;
        };
        if !matches!(target, PaneNode::Leaf { .. }) {
            // The path lands on a split. Conventionally, summon
            // wants a leaf anchor so the user's mental model of
            // "summon next to *this*" stays consistent.
            return false;
        }
        let placeholder = PaneNode::Leaf {
            pane_id: PaneId(0),
            content: PaneContent::Custom("__placeholder__".to_string()),
            graph_id: GraphId(uuid::Uuid::nil()),
        };
        let original = std::mem::replace(target, placeholder);
        let (first, second) = if side.new_is_first() {
            (Box::new(new_leaf), Box::new(original))
        } else {
            (Box::new(original), Box::new(new_leaf))
        };
        *target = PaneNode::Split {
            axis: side.split_axis(),
            ratio: 0.5,
            first,
            second,
        };
        true
    }

    /// Move the leaf at `source_path` to be adjacent to the leaf
    /// at `target_path`, on the indicated `side`. Preserves the
    /// source leaf's content + `pane_id` + `graph_id` — only its
    /// position in the tree changes, so per-pane state (camera,
    /// tiles, lineage tree) survives the move.
    ///
    /// Refuses to move (returns `false`):
    /// - When either path doesn't resolve to a leaf.
    /// - When `source_path == target_path` (no-op move).
    /// - When the target leaf is a descendant of the source's
    ///   parent in a way that would orphan nodes — currently
    ///   approximated by refusing moves where the source has only
    ///   one sibling (collapsing the split would leave nothing
    ///   adjacent to drop onto). Conservative; can be refined.
    ///
    /// Implementation: extract the source leaf (close + retain), then
    /// summon it adjacent to the target. Paths are recomputed because
    /// the close step may shift the tree.
    pub fn reparent_leaf(
        &mut self,
        source_path: &[SplitChoice],
        target_path: &[SplitChoice],
        side: InsertSide,
    ) -> bool {
        if source_path == target_path {
            return false;
        }
        // Extract the source leaf (we need an owned copy before the
        // close call mutates the tree).
        let Some(source_node) = walk(&self.root, source_path) else {
            return false;
        };
        let PaneNode::Leaf { .. } = source_node else {
            return false;
        };
        let source_clone = source_node.clone();

        // Identify the source leaf by `pane_id` so we can find it
        // post-close (the close may shift paths). Leaves are
        // uniquely identified by `pane_id` within a layout.
        let source_pane = if let PaneNode::Leaf { pane_id, .. } = &source_clone {
            *pane_id
        } else {
            return false;
        };

        // Pre-flight: confirm the target leaf exists + isn't the
        // source itself.
        let Some(PaneNode::Leaf {
            pane_id: target_pane,
            ..
        }) = walk(&self.root, target_path)
        else {
            return false;
        };
        if *target_pane == source_pane {
            return false;
        }
        let target_pane = *target_pane;

        // Close the source (its sibling is promoted). If close
        // refused (e.g., source is the root), bail.
        if !self.close_leaf(source_path) {
            return false;
        }

        // Re-find the target by pane_id (its path may have shifted
        // because of the close-collapse).
        let Some(new_target_path) = path_for_pane_id(&self.root, target_pane) else {
            // Target vanished (shouldn't happen — source and
            // target were distinct leaves). Bail; tree is now
            // inconsistent (source lost). Best-effort recovery is
            // out of scope for v0.
            return false;
        };

        // Re-attach the source adjacent to the (re-pathed) target.
        self.summon_leaf(&new_target_path, side, source_clone)
    }

    /// Close (remove) the leaf at `path`. Walks to the parent split,
    /// promotes the sibling leaf in its place — so the surrounding
    /// layout collapses naturally. Returns `true` if a leaf was
    /// removed; `false` if the path didn't resolve to a leaf or if
    /// the target is the root (a frame must have at least one leaf).
    pub fn close_leaf(&mut self, path: &[SplitChoice]) -> bool {
        if path.is_empty() {
            // Root leaf can't be removed (frame must keep a panel).
            return false;
        }
        let (parent_path, last_step) = path.split_at(path.len() - 1);
        let parent = walk_mut(&mut self.root, parent_path);
        let Some(parent) = parent else {
            return false;
        };
        let PaneNode::Split { first, second, .. } = parent else {
            return false;
        };
        let removed_first = last_step[0];
        let keeper = match removed_first {
            SplitChoice::First => std::mem::replace(
                second.as_mut(),
                PaneNode::Leaf {
                    pane_id: PaneId(0),
                    content: PaneContent::Custom("__placeholder__".to_string()),
                    graph_id: GraphId(uuid::Uuid::nil()),
                },
            ),
            SplitChoice::Second => std::mem::replace(
                first.as_mut(),
                PaneNode::Leaf {
                    pane_id: PaneId(0),
                    content: PaneContent::Custom("__placeholder__".to_string()),
                    graph_id: GraphId(uuid::Uuid::nil()),
                },
            ),
        };
        // Replace the parent split entirely with the surviving sibling.
        *parent = keeper;
        true
    }

    /// Re-point every **graph-bound** leaf (per
    /// [`PaneContent::follows_active_graph`]) at `graph`, leaving
    /// window-chrome leaves' `graph_id` untouched. This is the
    /// multi-graph "re-source the graph-bound panes" operation: the
    /// host calls it on a session switch so the orrery / roster /
    /// gloss / inspector / workbench panes follow the new active
    /// graph while the Steward / Comms / Apparatus panes stay put.
    /// (Multi-graph MG5; the model-B switch.)
    pub fn retag_graph_bound(&mut self, graph: GraphId) {
        fn walk(node: &mut PaneNode, graph: GraphId) {
            match node {
                PaneNode::Leaf {
                    content, graph_id, ..
                } => {
                    if content.follows_active_graph() {
                        *graph_id = graph;
                    }
                }
                PaneNode::Split { first, second, .. } => {
                    walk(first, graph);
                    walk(second, graph);
                }
            }
        }
        walk(&mut self.root, graph);
    }

    /// Re-source only the graph-bound leaves currently on `from` to `to`, leaving
    /// leaves bound to *other* graphs (a second graph-pane) and window-chrome
    /// leaves untouched. The pane-as-unit switch: a session switch re-points the
    /// panes that were showing the outgoing graph, not every graph-bound leaf, so
    /// a pane pinned to a different graph survives the switch. (Window composition
    /// — pane-as-unit; supersedes [`retag_graph_bound`] on the switch path, which
    /// stays for the initial all-leaves binding at restore.)
    pub fn retag_graph_bound_from(&mut self, from: GraphId, to: GraphId) {
        fn walk(node: &mut PaneNode, from: GraphId, to: GraphId) {
            match node {
                PaneNode::Leaf {
                    content, graph_id, ..
                } => {
                    if content.follows_active_graph() && *graph_id == from {
                        *graph_id = to;
                    }
                }
                PaneNode::Split { first, second, .. } => {
                    walk(first, from, to);
                    walk(second, from, to);
                }
            }
        }
        walk(&mut self.root, from, to);
    }

    /// Re-source the graph-bound leaves whose `graph_id` is **not** in `valid` (a
    /// nil / stale id from a persisted layout, or a graph whose session is gone) to
    /// `fallback`, leaving leaves pinned to a *valid* graph untouched. The restore
    /// path: a layout reloaded with a second graph-pane keeps that pane pinned (its
    /// graph is real), while genuinely stale leaves snap to the active graph. (MG5;
    /// Window composition — pane-as-unit restore.)
    pub fn retag_graph_bound_invalid(
        &mut self,
        valid: &std::collections::HashSet<GraphId>,
        fallback: GraphId,
    ) {
        fn walk(
            node: &mut PaneNode,
            valid: &std::collections::HashSet<GraphId>,
            fallback: GraphId,
        ) {
            match node {
                PaneNode::Leaf {
                    content, graph_id, ..
                } => {
                    if content.follows_active_graph() && !valid.contains(graph_id) {
                        *graph_id = fallback;
                    }
                }
                PaneNode::Split { first, second, .. } => {
                    walk(first, valid, fallback);
                    walk(second, valid, fallback);
                }
            }
        }
        walk(&mut self.root, valid, fallback);
    }

    /// Collapse duplicate graph panes: keep only the **first** Orrery leaf per
    /// `graph_id`, dropping any later one (its split folds into its sibling). Two
    /// spatial panes on one graph can't hold independent cameras yet, so a duplicate
    /// renders blank; this is the guardrail that stops them forming or accumulating
    /// across restores. Window-chrome leaves and distinct-graph panes are untouched.
    /// (Window composition — pane-as-unit; one Orrery pane per graph.)
    pub fn dedupe_graph_panes(&mut self) {
        let mut seen: std::collections::HashSet<GraphId> = std::collections::HashSet::new();
        dedup_node(&mut self.root, &mut seen);
    }

    /// Mutable access to one pane's content, by id. The seam a host needs to
    /// edit a leaf's OWN config in place — a Gloss pane's composed sections
    /// ride its [`PaneContent::Gloss`], so toggling one is a leaf edit, which
    /// is what makes the choice persist with the layout and travel with the
    /// pane on tear-out. `None` when no leaf carries that id.
    pub fn content_mut(&mut self, pane_id: PaneId) -> Option<&mut PaneContent> {
        let path = path_for_pane_id(&self.root, pane_id)?;
        match walk_mut(&mut self.root, &path)? {
            PaneNode::Leaf { content, .. } => Some(content),
            PaneNode::Split { .. } => None,
        }
    }

    /// Iterate every leaf in the layout in depth-first order
    /// (first-child before second-child). Yields `(pane_id, content,
    /// graph_id)` triples. Used by the host to assemble per-pane
    /// state + verify which graphs are referenced in this window.
    pub fn iter_leaves(&self) -> impl Iterator<Item = (PaneId, &PaneContent, GraphId)> {
        let mut out: Vec<(PaneId, &PaneContent, GraphId)> = Vec::new();
        fn walk<'a>(node: &'a PaneNode, out: &mut Vec<(PaneId, &'a PaneContent, GraphId)>) {
            match node {
                PaneNode::Leaf {
                    pane_id,
                    content,
                    graph_id,
                } => out.push((*pane_id, content, *graph_id)),
                PaneNode::Split { first, second, .. } => {
                    walk(first, out);
                    walk(second, out);
                }
            }
        }
        walk(&self.root, &mut out);
        out.into_iter()
    }
}

fn walk_mut<'a>(node: &'a mut PaneNode, path: &[SplitChoice]) -> Option<&'a mut PaneNode> {
    let mut current = node;
    for step in path {
        let PaneNode::Split { first, second, .. } = current else {
            return None;
        };
        current = match step {
            SplitChoice::First => first.as_mut(),
            SplitChoice::Second => second.as_mut(),
        };
    }
    Some(current)
}

/// Walk `node` left-to-right, recording each Orrery leaf's `graph_id` in `seen`;
/// when a child reduces to a duplicate Orrery leaf, fold the split into its other
/// child (the survivor is then re-examined, so a chain of duplicates collapses).
/// Returns `true` if `node` *itself* is a duplicate Orrery leaf the caller should
/// drop. (Backs [`FrisketLayout::dedupe_graph_panes`].)
fn dedup_node(node: &mut PaneNode, seen: &mut std::collections::HashSet<GraphId>) -> bool {
    match node {
        PaneNode::Leaf {
            content, graph_id, ..
        } => matches!(content, PaneContent::Orrery) && !seen.insert(*graph_id),
        PaneNode::Split { first, second, .. } => {
            // Process both children left-to-right (recording graph_ids, collapsing
            // their own duplicates) *before* folding, so the survivor is never
            // re-counted. Each child reduces to "drop me" only if it is itself a lone
            // duplicate Orrery leaf.
            let drop_first = dedup_node(first, seen);
            let drop_second = dedup_node(second, seen);
            match (drop_first, drop_second) {
                // Both children are duplicate leaves → this split collapses too.
                (true, true) => true,
                (true, false) => {
                    let keeper = std::mem::replace(second.as_mut(), dedup_placeholder());
                    *node = keeper;
                    false
                }
                (false, true) => {
                    let keeper = std::mem::replace(first.as_mut(), dedup_placeholder());
                    *node = keeper;
                    false
                }
                (false, false) => false,
            }
        }
    }
}

/// A throwaway leaf used as the `mem::replace` stand-in while promoting a split's
/// survivor over a dropped duplicate (never observed — the node is overwritten).
fn dedup_placeholder() -> PaneNode {
    PaneNode::Leaf {
        pane_id: PaneId(0),
        content: PaneContent::Custom("__dedup_placeholder__".to_string()),
        graph_id: GraphId(uuid::Uuid::nil()),
    }
}

/// Read-only walker — sibling of [`walk_mut`].
fn walk<'a>(node: &'a PaneNode, path: &[SplitChoice]) -> Option<&'a PaneNode> {
    let mut current = node;
    for step in path {
        let PaneNode::Split { first, second, .. } = current else {
            return None;
        };
        current = match step {
            SplitChoice::First => first.as_ref(),
            SplitChoice::Second => second.as_ref(),
        };
    }
    Some(current)
}

/// Find the `SplitPath` to the leaf with `pane_id`, if any. Used
/// by `reparent_leaf` to relocate the target after the close-step
/// shifts the tree.
///
/// `pub(crate)` so the test module (sibling of this module) can
/// exercise it directly; nothing outside the crate calls it.
pub(crate) fn path_for_pane_id(root: &PaneNode, pane_id: PaneId) -> Option<SplitPath> {
    fn descend(node: &PaneNode, path: &mut Vec<SplitChoice>, target: PaneId) -> Option<SplitPath> {
        match node {
            PaneNode::Leaf { pane_id, .. } if *pane_id == target => Some(path.clone()),
            PaneNode::Leaf { .. } => None,
            PaneNode::Split { first, second, .. } => {
                path.push(SplitChoice::First);
                if let Some(hit) = descend(first, path, target) {
                    return Some(hit);
                }
                path.pop();
                path.push(SplitChoice::Second);
                if let Some(hit) = descend(second, path, target) {
                    return Some(hit);
                }
                path.pop();
                None
            }
        }
    }
    let mut path = Vec::new();
    descend(root, &mut path, pane_id)
}
