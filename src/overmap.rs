//! The overmap (O0): the session set as a GRAPH, derived from the manifest
//! store — sessions are container nodes one level up (the overmap ruling,
//! recorded in mere's node-dissolution facets plan; rungs in the
//! `2026-07-20_overmap_sessions_graph_plan`).
//!
//! Derived, not stored: `GraphSessionManifest` already carries the overmap's
//! adjacency flat (`root_graph_id` the container id, `parent_session` a
//! lineage edge, `sub_graph_refs` containment), so this module just projects
//! it into a kernel [`Graph`] on demand — no second store, no sidecar. The
//! promotion gate (a stored chartulary overmap) stays consumer-pulled.
//!
//! Node identity IS the container id (`root_graph_id` — the same uuid the
//! session's `scene.*` facets key on; nil ids are healed at boot). Each node's
//! url is `mere://session/<session-id>` — the DOM-carried targeting key the
//! probe/automation doctrine wants — and its title is the session's display
//! label. Lineage renders as a provenance edge (child `CopiedFrom` parent,
//! matching the fork's per-node derivation records); `sub_graph_refs` as
//! containment (`CollectionMember`).

use mere::kernel::graph::apply::{self as graph_apply, GraphDelta, apply_graph_delta};
use mere::kernel::graph::{
    ContainmentSubKind, EdgeAssertion, Graph, NodeKey, ProvenanceSubKind,
};
use session_runtime::ManifestStore;
use std::collections::HashMap;

use frisket::SessionId;

/// The url scheme an overmap node carries: `mere://session/<uuid>`.
const SESSION_URL_PREFIX: &str = "mere://session/";

/// A session-node's url — the overmap's stable targeting key.
pub fn session_url(id: SessionId) -> String {
    format!("{SESSION_URL_PREFIX}{}", id.0)
}

/// Parse a session id back out of an overmap node url. `None` for any other
/// url (an overmap graph holds only session nodes today, but the parser stays
/// honest for the day it holds more).
pub fn session_of_url(url: &str) -> Option<SessionId> {
    let raw = url.strip_prefix(SESSION_URL_PREFIX)?;
    uuid::Uuid::parse_str(raw).ok().map(SessionId)
}

/// Build the overmap graph from the manifest store: one node per session
/// (keyed by its container id, titled by its label), a provenance edge per
/// lineage link (child `CopiedFrom` parent), a containment edge per
/// `sub_graph_refs` entry whose target is itself a session's container.
/// Positions are no graph truth here either — the view lays the overmap out
/// (lineage generations), like any other graph.
pub fn overmap_graph(sessions: &ManifestStore) -> Graph {
    let mut graph = Graph::new();
    let mut key_of: HashMap<SessionId, NodeKey> = HashMap::new();
    let mut container_key: HashMap<uuid::Uuid, NodeKey> = HashMap::new();

    for (id, manifest) in sessions.iter() {
        let container = *manifest.root_graph_id.as_uuid();
        // Post-heal every container id is real and unique; a duplicate (a
        // hand-edited profile) collapses onto the first-seen node rather than
        // corrupting the graph.
        if let Some(&key) = container_key.get(&container) {
            key_of.insert(id, key);
            continue;
        }
        let key = graph_apply::add_node(
            &mut graph,
            Some(container),
            session_url(id),
            Default::default(),
        );
        let label = manifest
            .display_name
            .clone()
            .unwrap_or_else(|| id.0.to_string()[..8].to_string());
        let _ = apply_graph_delta(&mut graph, GraphDelta::SetNodeTitle { key, title: label });
        key_of.insert(id, key);
        container_key.insert(container, key);
    }

    for (id, manifest) in sessions.iter() {
        let Some(&child) = key_of.get(&id) else { continue };
        // Lineage: the child was forked from the parent — the same CopiedFrom
        // sense the fork stamps on each copied node, one level up.
        if let Some(parent) = manifest.parent_session
            && let Some(&parent_key) = key_of.get(&parent)
        {
            let _ = graph_apply::assert_relation(
                &mut graph,
                child,
                parent_key,
                EdgeAssertion::Provenance {
                    sub_kind: ProvenanceSubKind::CopiedFrom,
                },
            );
        }
        // Containment: a sub-graph ref whose target is some session's
        // container draws parent-contains-child.
        for sub in &manifest.sub_graph_refs {
            if let Some(&sub_key) = container_key.get(sub.as_uuid()) {
                let _ = graph_apply::assert_relation(
                    &mut graph,
                    child,
                    sub_key,
                    EdgeAssertion::Containment {
                        sub_kind: ContainmentSubKind::CollectionMember,
                    },
                );
            }
        }
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use frisket::GraphId;
    use session_runtime::GraphSessionManifest;

    fn store_with(sessions: Vec<GraphSessionManifest>) -> ManifestStore {
        let mut store = ManifestStore::new();
        for m in sessions {
            store.insert(m);
        }
        store
    }

    #[test]
    fn sessions_project_as_container_nodes_with_lineage() {
        let donor = SessionId::new();
        let fork = SessionId::new();
        let donor_container = GraphId::from_uuid(uuid::Uuid::from_u128(0xd0));
        let fork_container = GraphId::from_uuid(uuid::Uuid::from_u128(0xf0));
        let mut donor_m = GraphSessionManifest::new(donor, donor_container);
        donor_m.display_name = Some("home".to_string());
        let mut fork_m = GraphSessionManifest::new(fork, fork_container);
        fork_m.parent_session = Some(donor);

        let graph = overmap_graph(&store_with(vec![donor_m, fork_m]));
        assert_eq!(graph.nodes().count(), 2);
        // Node identity is the container id; url carries the session id.
        let (donor_key, donor_node) = graph
            .get_node_by_id(uuid::Uuid::from_u128(0xd0))
            .expect("donor keyed by container");
        assert_eq!(donor_node.url(), session_url(donor));
        assert_eq!(session_of_url(donor_node.url()), Some(donor));
        assert_eq!(graph.node_display_label(donor_key), "home");
        // The lineage edge: fork CopiedFrom donor, one provenance relation.
        let lineage: Vec<_> = graph.relations().collect();
        assert_eq!(lineage.len(), 1);
        let (fork_key, _) = graph.get_node_by_id(uuid::Uuid::from_u128(0xf0)).unwrap();
        assert_eq!(lineage[0].from, fork_key);
        assert_eq!(lineage[0].to, donor_key);
    }

    #[test]
    fn a_label_less_session_titles_by_id_prefix() {
        let id = SessionId::new();
        let m = GraphSessionManifest::new(id, GraphId::from_uuid(uuid::Uuid::from_u128(0xa1)));
        let graph = overmap_graph(&store_with(vec![m]));
        let (key, _) = graph.get_node_by_id(uuid::Uuid::from_u128(0xa1)).unwrap();
        assert_eq!(graph.node_display_label(key), id.0.to_string()[..8].to_string());
    }

    #[test]
    fn session_of_url_rejects_foreign_urls() {
        assert!(session_of_url("https://example.com").is_none());
        assert!(session_of_url("mere://session/not-a-uuid").is_none());
    }

    #[test]
    fn duplicate_container_ids_collapse_rather_than_corrupt() {
        // A hand-edited (or unhealed) profile: two sessions, one container id.
        let shared = GraphId::from_uuid(uuid::Uuid::from_u128(0xcc));
        let a = GraphSessionManifest::new(SessionId::new(), shared);
        let b = GraphSessionManifest::new(SessionId::new(), shared);
        let graph = overmap_graph(&store_with(vec![a, b]));
        assert_eq!(graph.nodes().count(), 1, "first-seen wins; no corruption");
    }
}
