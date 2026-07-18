//! Project a [`FrisketLayout`] into a uxtree subtree. Split out of
//! `lib.rs` to keep the parent module under the workspace's 600-LOC
//! ceiling.

use accesskit::{Node, Role};
use uxtree::{UxTree, node_id_for_path};

use crate::{FrisketLayout, PaneContent, PaneId, PaneNode};

/// Project a [`FrisketLayout`] into a uxtree subtree describing the
/// pane structure.
///
/// Splits become `Role::Group` nodes annotated with axis + ratio in
/// their description. Leaves become `Role::Group` nodes labeled by
/// their `PaneContent` tag. The leaf nodes are addressable by stable
/// id (derived from their `PaneId`); the host can use those ids to
/// stitch each leaf's content subtree (workbench / orrery / …) under
/// the corresponding leaf node, or render content separately while
/// keeping uxtree structurally aware of the layout.
pub fn project_frisket(layout: &FrisketLayout) -> UxTree {
    project_frisket_with(layout, |_, _| None)
}

/// Project a frame layout, calling `content_for` at each leaf to ask
/// for a content subtree to attach. The returned subtree's root becomes
/// the leaf's accesskit child; its nodes are merged into the frame's
/// node list. Resolver returning `None` leaves the leaf empty (same as
/// [`project_frisket`]).
///
/// Use this when the host wants the frame's leaf nodes to actually
/// carry their content's a11y / automation tree (workbench in pane 1,
/// orrery in pane 2, …) rather than tracking parallel subtrees.
pub fn project_frisket_with<F>(layout: &FrisketLayout, mut content_for: F) -> UxTree
where
    F: FnMut(&PaneContent, PaneId) -> Option<UxTree>,
{
    let mut nodes = Vec::new();
    let root_path = format!("frisket/{}", layout.id.as_str());
    let root_id = node_id_for_path(&root_path);

    let root_child = project_node(&layout.root, &root_path, &mut nodes, &mut content_for);

    let mut root = Node::new(Role::Group);
    root.set_label(layout.label.clone());
    root.set_children(vec![root_child]);
    nodes.push((root_id, root));

    tracing::debug!(
        frame_id = %layout.id.as_str(),
        node_count = nodes.len(),
        "projected FrisketLayout into uxtree subtree"
    );

    UxTree {
        root: root_id,
        nodes,
    }
}

fn project_node<F>(
    node: &PaneNode,
    path: &str,
    nodes: &mut Vec<(accesskit::NodeId, Node)>,
    content_for: &mut F,
) -> accesskit::NodeId
where
    F: FnMut(&PaneContent, PaneId) -> Option<UxTree>,
{
    match node {
        PaneNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let split_path = format!("{path}/split");
            let split_id = node_id_for_path(&split_path);
            let first_id = project_node(first, &format!("{split_path}/first"), nodes, content_for);
            let second_id =
                project_node(second, &format!("{split_path}/second"), nodes, content_for);

            let mut split_node = Node::new(Role::Group);
            split_node.set_description(format!("split {:?} ratio={ratio:.2}", axis));
            split_node.set_children(vec![first_id, second_id]);
            nodes.push((split_id, split_node));
            split_id
        }
        PaneNode::Leaf {
            pane_id, content, ..
        } => {
            let leaf_path = format!("{path}/pane/{}", pane_id.0);
            let leaf_id = node_id_for_path(&leaf_path);

            let mut leaf_children = Vec::new();
            if let Some(content_tree) = content_for(content, *pane_id) {
                leaf_children.push(content_tree.root);
                nodes.extend(content_tree.nodes);
            }

            let mut leaf_node = Node::new(Role::Group);
            leaf_node.set_label(content.tag().to_string());
            leaf_node.set_children(leaf_children);
            nodes.push((leaf_id, leaf_node));
            leaf_id
        }
    }
}
