//! The Roster pane's rows: the graph's node manifest, off graph truth. Rung 5
//! slice D — the second real pane, after Trail.
//!
//! `mere::roster` is the neutral vocabulary (`NodeRowInput` in, content-bucketed
//! `RosterRow` out, on the P8 pattern). The Roster is the data-view counterpart
//! to the canvas's space-view: every node, sectioned by content type. This host
//! half gathers the inputs from the canvas graph and flattens the rows (a section
//! header, then its nodes) into what the pane renders and a click navigates to.
//! Unlike Trail's neutral `Row`, `RosterRow` carries the url, so no re-attaching.
//!
//! Slice D renders the Nodes tab (the manifest's default). The Links, Graphlets,
//! and Fields tabs `mere::roster` also models are a follow-on.

use mere::roster::{NodeRowInput, build_node_rows};

use crate::app::App;
use crate::content::NodeContent;

/// How the pane renders a row and what a click on it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterRowAction {
    /// A content-type section header ("Web page", "Image", ...). Inert.
    Header,
    /// A node row; a click navigates to its url (through `Action::OpenAddress`).
    Node(String),
}

/// One Roster row: display text, the affordance behind it, and whether it is the
/// selected node (rendered highlighted).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterViewRow {
    pub text: String,
    pub action: RosterRowAction,
    pub selected: bool,
}

/// Gather the Roster's Nodes rows from the canvas graph: every node, sectioned
/// by content type, the focused node marked selected and any live node marked
/// open. Pure over the app's read-only graph.
pub fn roster_rows(app: &App) -> Vec<RosterViewRow> {
    let graph = app.canvas.graph();
    let focused = app.canvas.focused_member();

    let inputs: Vec<NodeRowInput> = graph
        .nodes()
        .map(|(key, node)| NodeRowInput {
            member: node.id,
            title: graph.node_display_label(key),
            url: node.url().to_string(),
            content_type: node.mime_hint.clone(),
            tags: node.tags.iter().cloned().collect(),
            selected: focused == Some(node.id),
            open: matches!(app.content.get(node.id), Some(NodeContent::Live)),
        })
        .collect();

    // Flatten: build_node_rows buckets by content type and stamps the first row
    // of each bucket with a section header. Emit the header as its own inert row,
    // then the node row.
    let mut out = Vec::new();
    for row in build_node_rows(inputs) {
        if let Some(header) = row.section_header {
            out.push(RosterViewRow {
                text: header,
                action: RosterRowAction::Header,
                selected: false,
            });
        }
        out.push(RosterViewRow {
            text: row.title,
            action: RosterRowAction::Node(row.url),
            selected: row.selected,
        });
    }
    out
}

/// The row a point at pane-local `y` falls on. Shared list-pane geometry.
pub fn row_at(rows: &[RosterViewRow], local_y: f32) -> Option<usize> {
    crate::pane_rows::row_index_at(rows.len(), local_y)
}

/// One flat node row for the cambium `data_grid` (rung 5 slice D toolkit
/// adoption). The grid is columned, not sectioned — the content-type buckets
/// `roster_rows` renders as headers become a grid column instead — so this drops
/// the section headers and keeps the node's fields per column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterGridRow {
    pub title: String,
    pub kind: String,
    pub url: String,
    pub selected: bool,
}

/// Gather the Roster's node rows as flat grid rows (no section headers), for the
/// cambium `data_grid`. Same graph-truth gather and `build_node_rows` sort as
/// `roster_rows`; only the shaping differs.
pub fn roster_grid_rows(app: &App) -> Vec<RosterGridRow> {
    let graph = app.canvas.graph();
    let focused = app.canvas.focused_member();
    let inputs: Vec<NodeRowInput> = graph
        .nodes()
        .map(|(key, node)| NodeRowInput {
            member: node.id,
            title: graph.node_display_label(key),
            url: node.url().to_string(),
            content_type: node.mime_hint.clone(),
            tags: node.tags.iter().cloned().collect(),
            selected: focused == Some(node.id),
            open: matches!(app.content.get(node.id), Some(NodeContent::Live)),
        })
        .collect();
    build_node_rows(inputs)
        .into_iter()
        .map(|r| RosterGridRow {
            title: r.title,
            kind: r.content_type.unwrap_or_else(|| "—".to_string()),
            url: r.url,
            selected: r.selected,
        })
        .collect()
}
