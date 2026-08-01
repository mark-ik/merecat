//! The shared graph, as the app sees it.
//!
//! Between the authority-filtered Commons fold and the Canvas a person looks
//! at. Data only: addresses and labels, no store handles, no operations, no
//! keys. What arrives here has already passed the authority filter, so
//! nothing pending or revoked can reach a surface through this path.
//!
//! ## Why reconciliation is additive
//!
//! A session's Canvas is the person's own workspace. Shared nodes arrive into
//! it, but the two are not the same graph and the shared side is not
//! authoritative over the local one: it decides what the *place* holds, never
//! what this person's canvas holds. So reconciliation adds what is missing and
//! marks what is shared, and never removes, moves, or relabels a node the
//! person put there. A shared node that disappears from the place stops being
//! marked as shared; it does not vanish from under the cursor.

use serde::{Deserialize, Serialize};

/// One node the shared graph holds, reduced to what a surface needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedNode {
    /// The Commons container id, hex. Stable across peers, which is what makes
    /// this reconcilable rather than merely displayable.
    pub id: String,
    /// The address this node names, when it names one.
    pub address: String,
}

/// The shared graph's product-visible content.
///
/// Separate from `GraphCache`'s counts rather than replacing them: counts
/// answer "is anything pending or revoked", this answers "what is here". A
/// surface needs both and they change at different rates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedGraph {
    pub nodes: Vec<SharedNode>,
}

impl SharedGraph {
    /// Reduce an authority-filtered Commons fold to product-visible content.
    ///
    /// Takes the already-filtered projection, never the raw one: everything
    /// pending or revoked is gone before this sees it, so there is no path
    /// from an unauthorized operation to a surface through here.
    pub(crate) fn from_projection(projection: &commons::CommonsProjection) -> Self {
        let mut nodes: Vec<SharedNode> = projection
            .graph
            .graph()
            .nodes()
            .map(|(_, container)| SharedNode {
                id: container.id.clone(),
                address: container
                    .addresses
                    .first()
                    .map(|address| address.as_str().to_string())
                    .unwrap_or_default(),
            })
            .collect();
        // Deterministic order, so two peers holding the same place present it
        // the same way and a diff between them means a real difference.
        nodes.sort_by(|a, b| a.id.cmp(&b.id));
        Self { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Addresses in the shared graph, for a caller reconciling by address.
    pub fn addresses(&self) -> impl Iterator<Item = &str> {
        self.nodes
            .iter()
            .map(|node| node.address.as_str())
            .filter(|address| !address.is_empty())
    }
}
