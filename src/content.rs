//! The content lane's app truth (rung 4, born ahead of its port): per-node
//! document lifecycle only. Charter per the architecture plan's module map:
//! engine registrations, per-node document lifecycle, the verso-tile flip,
//! content frames, and input routing — where the registry itself is
//! genet/inker's, never a hand-wired lane ladder (the session-engines plan,
//! genet docs 2026-07-10, phase 4 names merecat as its consumer).
//!
//! What lives HERE is the lifecycle state machine and nothing else. Live
//! document sessions are retained, non-`Send` handles, so the shell's
//! content port owns them (ports are the only owners of handles; `App`
//! holds data) keyed by the same node ids. Until the genet-documents
//! component lands (engines plan phase 2), the shell port answers every
//! spawn with an honest failure naming the gap — a `Requested` node never
//! silently spins (the no-placebo rule).

use std::collections::HashMap;

use uuid::Uuid;

/// One node's content lifecycle. Absent from the map = no content activity
/// (the at-rest state for every node).
#[derive(Clone, Debug, PartialEq)]
pub enum NodeContent {
    /// A spawn effect is in flight through the content port.
    Requested,
    /// A live session exists shell-side; frames compose into the node.
    Live,
    /// The last spawn failed; the reason is surfaced, never swallowed.
    Failed(String),
}

/// The app-truth side of the content lane: node id -> lifecycle.
#[derive(Debug, Default)]
pub struct ContentStates {
    states: HashMap<Uuid, NodeContent>,
}

impl ContentStates {
    pub fn get(&self, node: Uuid) -> Option<&NodeContent> {
        self.states.get(&node)
    }

    /// Whether a flip intent on `node` should spawn (true) or close (false):
    /// live and in-flight content toggles OFF; empty and failed toggle ON
    /// (a failed node retries — failure is a state, not a latch).
    pub fn flip_spawns(&self, node: Uuid) -> bool {
        !matches!(
            self.states.get(&node),
            Some(NodeContent::Live | NodeContent::Requested)
        )
    }

    pub fn note_requested(&mut self, node: Uuid) {
        self.states.insert(node, NodeContent::Requested);
    }

    pub fn note_live(&mut self, node: Uuid) {
        self.states.insert(node, NodeContent::Live);
    }

    pub fn note_failed(&mut self, node: Uuid, error: String) {
        self.states.insert(node, NodeContent::Failed(error));
    }

    /// The node's content is gone (closed, or the port dropped it).
    pub fn note_closed(&mut self, node: Uuid) {
        self.states.remove(&node);
    }

    /// Nodes currently holding live sessions (the shell composes these).
    pub fn live_nodes(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.states
            .iter()
            .filter(|(_, s)| matches!(s, NodeContent::Live))
            .map(|(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_toggles_and_failure_retries() {
        let node = Uuid::new_v4();
        let mut states = ContentStates::default();
        assert!(states.flip_spawns(node), "empty flips ON");
        states.note_requested(node);
        assert!(!states.flip_spawns(node), "in-flight flips OFF, not double-spawns");
        states.note_live(node);
        assert!(!states.flip_spawns(node), "live flips OFF");
        states.note_closed(node);
        assert!(states.flip_spawns(node), "closed flips ON again");
        states.note_requested(node);
        states.note_failed(node, "no port".into());
        assert!(states.flip_spawns(node), "failed retries on the next flip");
    }

    #[test]
    fn live_nodes_lists_only_live() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut states = ContentStates::default();
        states.note_live(a);
        states.note_requested(b);
        states.note_failed(c, "x".into());
        let live: Vec<_> = states.live_nodes().collect();
        assert_eq!(live, vec![a]);
    }
}
