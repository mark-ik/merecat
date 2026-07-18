//! The accessibility projection (rung 5 slice F): one stitched application
//! tree over app truth — chrome, the frisket pane structure, the workbench
//! tiling, and each live document's structural outline — with stable,
//! path-derived AccessKit ids (uxtree's scheme, so subtree ids stay disjoint
//! by construction).
//!
//! Supply, all pre-existing and previously unused: `frisket::project_frisket_with`
//! (the pane tree, with a per-leaf content hook), `mere::workbench::project_workbench`
//! (the tiling), and `uxtree::stitch`. The document subtree is built here from
//! the [`StructureFacts`] outline the content port mirrors at spawn (the
//! `DocumentSession::inspect` accessor landed with the Inspector slice) — so
//! the whole projection is PURE over `App`, testable headless, and the same
//! read the observation snapshot serves.
//!
//! Honesty (the no-placebo rule): this is `A11yCapability::Partial` by
//! declaration. The projection is structural — roles, names, and levels, no
//! bounds and no per-element focus — and each document root SAYS so in its
//! description rather than implying coverage it does not have. Pushing the
//! `TreeUpdate` to an OS adapter is a separate, later piece of work (the
//! donor never landed it either); producing the coherent tree is this rung's
//! deletion-matrix bar.

use accesskit::{Node, Role};
use frisket::PaneContent;
use uxtree::{UxTree, node_id_for_path, stitch};

use crate::app::App;
use crate::content::{NodeContent, OutlineFact};

/// Project the whole application into one stitched [`UxTree`]: a window root
/// whose children are the chrome subtree and the frisket pane tree, with the
/// workbench tiling under its pane leaf and a document subtree under the
/// canvas leaf for every node with live content.
pub fn project_app(app: &App) -> UxTree {
    let chrome = project_chrome(app);
    // Live documents, in graph order (deterministic), each from its mirrored
    // structural outline. They stitch under the CANVAS leaf: content insets
    // and workbench tiles alike render over graph truth, and a document
    // without a structural read is announced without children (honest).
    let docs: Vec<UxTree> = app
        .canvas
        .graph()
        .nodes()
        .filter(|(_, n)| matches!(app.content.get(n.id), Some(NodeContent::Live)))
        .map(|(_, n)| project_live_document(app, n.id, n.url()))
        .collect();
    let mut docs = Some(docs);
    let panes = frisket::project_frisket_with(&app.frisket, |content, _id| match content {
        PaneContent::Workbench => Some(mere::workbench::project_workbench(&app.workbench)),
        PaneContent::Orrery => {
            // The canvas leaf carries the graph summary plus the live
            // documents (their pixels composite over the canvas region).
            let count = app.canvas.graph().nodes().count();
            let mut root = Node::new(Role::Group);
            root.set_label(format!("graph canvas, {count} nodes"));
            let subtrees = docs.take().unwrap_or_default();
            Some(stitch("merecat/canvas", root, subtrees))
        }
        _ => None,
    });
    let mut root = Node::new(Role::Window);
    root.set_label("Merecat");
    stitch("merecat", root, vec![chrome, panes])
}

/// The projection flattened to "role: label" lines for the observation
/// snapshot (what `assert a11y` matches). Values fold into the line when a
/// node has one (the omnibar's text, a link's target).
pub fn a11y_lines(app: &App) -> Vec<String> {
    project_app(app)
        .nodes
        .iter()
        .map(|(_, n)| {
            let role = format!("{:?}", n.role()).to_lowercase();
            match (n.label(), n.value()) {
                (Some(label), Some(value)) => format!("{role}: {label} = {value}"),
                (Some(label), None) => format!("{role}: {label}"),
                (None, Some(value)) => format!("{role} = {value}"),
                (None, None) => role,
            }
        })
        .collect()
}

/// The chrome subtree: the omnibar (a text input when open, with its live
/// text and caret-free honesty) and the at-rest caption.
fn project_chrome(app: &App) -> UxTree {
    let mut nodes = Vec::new();
    let mut children = Vec::new();
    if app.omnibar.open {
        let id = node_id_for_path("merecat/chrome/omnibar");
        let mut n = Node::new(Role::TextInput);
        n.set_label("omnibar");
        n.set_value(app.omnibar.text.clone());
        nodes.push((id, n));
        children.push(id);
    }
    if let Some(caption) = crate::app::focused_caption(&app.canvas) {
        let id = node_id_for_path("merecat/chrome/caption");
        let mut n = Node::new(Role::Label);
        n.set_label(caption);
        nodes.push((id, n));
        children.push(id);
    }
    let root_id = node_id_for_path("merecat/chrome");
    let mut root = Node::new(Role::Group);
    root.set_label("chrome");
    root.set_children(children);
    nodes.push((root_id, root));
    UxTree {
        root: root_id,
        nodes,
    }
}

/// One live document's subtree, from the mirrored structural outline. The
/// root announces as a document and DECLARES the partial capability; outline
/// entries become flat children with mapped roles (depth is structural
/// bookkeeping — nesting reconstruction is a follow-on, said in the
/// description rather than faked).
fn project_live_document(app: &App, node: uuid::Uuid, url: &str) -> UxTree {
    let root_path = format!("merecat/doc/{node}");
    let root_id = node_id_for_path(&root_path);
    let mut nodes = Vec::new();
    let mut children = Vec::new();
    let structure = app.content.facts(node).and_then(|f| f.structure.as_ref());
    if let Some(s) = structure {
        for (i, entry) in s.outline.iter().enumerate() {
            let id = node_id_for_path(&format!("{root_path}/outline/{i}"));
            let mut n = Node::new(outline_role(entry));
            if !entry.name.is_empty() {
                n.set_label(entry.name.clone());
            }
            nodes.push((id, n));
            children.push(id);
        }
    }
    let mut root = Node::new(Role::Document);
    match structure.and_then(|s| s.title.as_deref()) {
        Some(title) => root.set_label(title.to_string()),
        None => root.set_label(url.to_string()),
    }
    // The honest capability declaration, on the node itself.
    root.set_description(match structure {
        Some(_) => "structural outline only: no bounds, no per-element focus",
        None => "no structural read for this lane",
    });
    root.set_children(children);
    nodes.push((root_id, root));
    UxTree {
        root: root_id,
        nodes,
    }
}

/// Map the outline's coarse role strings (genet's `role_of`) onto AccessKit
/// roles. Unknown strings stay groups.
fn outline_role(entry: &OutlineFact) -> Role {
    match entry.role {
        "link" => Role::Link,
        "button" => Role::Button,
        "textbox" => Role::TextInput,
        "paragraph" => Role::Paragraph,
        "heading" => Role::Heading,
        "list" => Role::List,
        "listitem" => Role::ListItem,
        "image" => Role::Image,
        "label" => Role::Label,
        "navigation" => Role::Navigation,
        "banner" => Role::Banner,
        "contentinfo" => Role::ContentInfo,
        "main" => Role::Main,
        "region" => Role::Section,
        _ => Role::Group,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::action::{Action, Update};
    use crate::content::{ContentFacts, StructureFacts};

    fn app_with_live_doc() -> App {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/".to_string()));
        let node = app.canvas.focused_member().unwrap();
        app.update(Action::OpenInWorkbench);
        app.apply_update(Update::ContentSpawned {
            node,
            facts: Some(ContentFacts {
                engine: "genet.web".to_string(),
                structure: Some(StructureFacts {
                    title: Some("Example Domain".to_string()),
                    headings: 1,
                    links: 1,
                    outline: vec![
                        OutlineFact {
                            depth: 0,
                            role: "heading",
                            name: "Example Domain".to_string(),
                        },
                        OutlineFact {
                            depth: 1,
                            role: "link",
                            name: "More information...".to_string(),
                        },
                    ],
                }),
            }),
        });
        app
    }

    /// The stitched tree is coherent: one window root, every id unique
    /// (disjoint subtree ranges — the deletion-matrix bar), and every
    /// child id present in the node list.
    #[test]
    fn the_stitched_tree_is_coherent() {
        let app = app_with_live_doc();
        let tree = project_app(&app);
        let ids: Vec<_> = tree.nodes.iter().map(|(id, _)| *id).collect();
        let unique: HashSet<_> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "no id collisions across subtrees");
        for (id, n) in &tree.nodes {
            for child in n.children() {
                assert!(
                    unique.contains(child),
                    "node {id:?} references a missing child {child:?}"
                );
            }
        }
        let (_, root) = tree.nodes.iter().find(|(id, _)| *id == tree.root).unwrap();
        assert_eq!(root.role(), Role::Window);
        assert_eq!(root.label(), Some("Merecat"));
    }

    /// The pane structure, the workbench tiling, and the live document's
    /// outline all arrive under the one root, and the document declares its
    /// partial capability instead of implying coverage.
    #[test]
    fn panes_workbench_and_documents_project_under_one_root() {
        let app = app_with_live_doc();
        let lines = a11y_lines(&app);
        let has = |s: &str| lines.iter().any(|l| l.contains(s));
        assert!(has("group: workbench"), "{lines:?}");
        assert!(has("tab: Tile"), "the tile's tab projects");
        assert!(has("document: Example Domain"), "{lines:?}");
        assert!(has("heading: Example Domain"));
        assert!(has("link: More information..."));
        assert!(has("graph canvas, 1 nodes"));
        let doc = project_app(&app)
            .nodes
            .into_iter()
            .map(|(_, n)| n)
            .find(|n| n.role() == Role::Document)
            .expect("the live document projects");
        assert_eq!(
            doc.description(),
            Some("structural outline only: no bounds, no per-element focus"),
            "the capability is declared, not implied"
        );
    }

    /// The omnibar joins the chrome subtree only while it is open, carrying
    /// its live text.
    #[test]
    fn the_omnibar_projects_while_open() {
        let mut app = App::test_stub();
        assert!(
            !a11y_lines(&app).iter().any(|l| l.starts_with("textinput")),
            "closed omnibar projects nothing"
        );
        app.update(Action::OmnibarOpen { command: false });
        app.update(Action::OmnibarChar('h'));
        app.update(Action::OmnibarChar('i'));
        let lines = a11y_lines(&app);
        assert!(
            lines.iter().any(|l| l == "textinput: omnibar = hi"),
            "{lines:?}"
        );
    }
}
