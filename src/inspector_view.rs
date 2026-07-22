//! The Inspector pane's data half: focused-object detail sections off app
//! truth (rung 5 slice D, last pane; merecat's outright per the pane
//! taxonomy — fetch/content introspection is app-runtime truth, so there is
//! no mere crate and none is wanted). The behavioral donor is meerkat's
//! `inspector.rs`, re-cut for merecat's shape: node facts come from the
//! kernel node, content facts from the [`ContentFacts`] mirror the content
//! port stamps at spawn (the structural read arrives through genet's
//! `DocumentSession::inspect`, the trait accessor this pane pulled into
//! existence).
//!
//! Pure functions of `App`; the view half ([`crate::inspector_pane`])
//! renders these sections on cambium's `detail_panel`, and the observation
//! snapshot flattens them so a scenario can assert a row.
//!
//! Not carried from meerkat (deliberately): the re-parse of the fetched
//! body through nematic engines — merecat's structural read comes from the
//! LIVE session, not a second parse (one truth, no drift). The sidecar rows
//! (viewer override, compat mode) joined at rung 6 with the browser-state
//! sidecar itself.

use crate::app::App;
use crate::content::NodeContent;

/// One section: a header and its key/value rows. The shape cambium's
/// `DetailSection` takes, kept as plain data here so this module stays
/// toolkit-free (the P8 pattern: explicit inputs in, neutral items out).
#[derive(Clone, Debug, PartialEq)]
pub struct InspectorSection {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// The Inspector's sections for the current app state.
pub fn inspector_sections(app: &App) -> Vec<InspectorSection> {
    let focused = app
        .canvas
        .focused_member()
        .and_then(|member| app.canvas.graph().nodes().map(|(_, n)| n).find(|n| n.id == member));

    let node_rows = match focused {
        Some(node) => vec![
            ("Title".to_string(), display_title(node.title.trim(), node.url())),
            ("URL".to_string(), node.url().to_string()),
            ("Node id".to_string(), node.id.to_string()),
            ("Addresses".to_string(), node.addresses.len().to_string()),
            ("Pinned".to_string(), yes_no(node.is_pinned)),
            (
                "Tags".to_string(),
                summarize_strings(node.tags.iter().map(String::as_str)),
            ),
            (
                "Import provenance".to_string(),
                summarize_import_provenance(node),
            ),
            (
                "Classifications".to_string(),
                summarize_classifications(node),
            ),
            (
                "Mime hint".to_string(),
                node.mime_hint.as_deref().unwrap_or("none").to_string(),
            ),
            // The sidecar rows (rung 6): the browser's handling of the node.
            (
                "Viewer".to_string(),
                app.browser
                    .get(node.id)
                    .and_then(|b| b.viewer_override.as_deref())
                    .unwrap_or("auto")
                    .to_string(),
            ),
            (
                "Compat".to_string(),
                yes_no(app.browser.get(node.id).is_some_and(|b| b.compat_mode)),
            ),
        ],
        None => vec![("Focused node".to_string(), "none".to_string())],
    };

    let content_rows = content_rows(app, focused.map(|n| n.id));

    vec![
        InspectorSection {
            title: "Node".to_string(),
            rows: node_rows,
        },
        InspectorSection {
            title: "Content".to_string(),
            rows: content_rows,
        },
        InspectorSection {
            title: "Journal".to_string(),
            rows: journal_rows(app),
        },
    ]
}

/// The attributed edit spine's tail, newest first (participant gate B1: WHO
/// changed the graph, readable). The author renders as the resident's label
/// when the subject hex matches a denizen, `you` for the UI author.
fn journal_rows(app: &App) -> Vec<(String, String)> {
    let Ok(journal) = app.journal.lock() else {
        return Vec::new();
    };
    journal
        .entries()
        .iter()
        .rev()
        .take(5)
        .map(|entry| {
            let author = if entry.author == mere::kernel::graph::USER_AUTHOR {
                "you".to_string()
            } else {
                app.denizens
                    .residents
                    .values()
                    .find(|r| r.subject.to_hex() == entry.author)
                    .map(|r| r.label.clone())
                    .unwrap_or_else(|| entry.author[..8.min(entry.author.len())].to_string())
            };
            let debug = format!("{:?}", entry.delta);
            let kind = debug
                .split(|c: char| c == ' ' || c == '{' || c == '(')
                .next()
                .unwrap_or("edit")
                .to_string();
            (author, kind)
        })
        .collect()
}

/// The sections flattened to "Key: value" lines (the observation snapshot's
/// rendering; `assert row` matches against these).
pub fn inspector_lines(app: &App) -> Vec<String> {
    inspector_sections(app)
        .into_iter()
        .flat_map(|s| s.rows)
        .map(|(k, v)| format!("{k}: {v}"))
        .collect()
}

fn content_rows(app: &App, node: Option<uuid::Uuid>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let state = node.and_then(|n| app.content.get(n));
    rows.push((
        "Content state".to_string(),
        match state {
            Some(NodeContent::Requested) => "requested".to_string(),
            Some(NodeContent::Live) => "live".to_string(),
            Some(NodeContent::Failed(err)) => format!("failed: {}", truncate(err, 120)),
            None => "none".to_string(),
        },
    ));
    let Some(facts) = node.and_then(|n| app.content.facts(n)) else {
        return rows;
    };
    rows.push(("Engine".to_string(), facts.engine.clone()));
    match &facts.structure {
        Some(s) => {
            rows.push((
                "Document title".to_string(),
                s.title.as_deref().unwrap_or("none").to_string(),
            ));
            rows.push((
                "Document structure".to_string(),
                format!("headings={} outline={}", s.headings, s.outline.len()),
            ));
            rows.push(("Outgoing links".to_string(), s.links.to_string()));
        }
        // The lane has no structural read: said, not synthesized.
        None => rows.push((
            "Structural read".to_string(),
            "none for this lane".to_string(),
        )),
    }
    rows
}

fn display_title(title: &str, url: &str) -> String {
    if title.is_empty() {
        url.to_string()
    } else {
        title.to_string()
    }
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// Up to four values, sorted, "+N" for the rest (the donor's shape).
fn summarize_strings<'a>(values: impl Iterator<Item = &'a str>) -> String {
    let mut values: Vec<_> = values.filter(|v| !v.trim().is_empty()).collect();
    values.sort_unstable();
    if values.is_empty() {
        return "none".to_string();
    }
    let shown = values.iter().take(4).copied().collect::<Vec<_>>().join(", ");
    if values.len() > 4 {
        format!("{shown}, +{}", values.len() - 4)
    } else {
        shown
    }
}

fn summarize_import_provenance(node: &mere::kernel::graph::Node) -> String {
    if node.import_provenance.is_empty() {
        return "none".to_string();
    }
    summarize_strings(node.import_provenance.iter().map(|p| {
        if p.source_label.is_empty() {
            p.source_id.as_str()
        } else {
            p.source_label.as_str()
        }
    }))
}

fn summarize_classifications(node: &mere::kernel::graph::Node) -> String {
    if node.classifications.is_empty() {
        return "none".to_string();
    }
    let labels = node
        .classifications
        .iter()
        .map(|c| c.label.as_deref().unwrap_or(c.value.as_str()));
    format!("{} ({})", node.classifications.len(), summarize_strings(labels))
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{Action, Update};
    use crate::content::{ContentFacts, StructureFacts};

    #[test]
    fn no_focus_reports_none_honestly() {
        let app = App::test_stub();
        let sections = inspector_sections(&app);
        assert_eq!(sections[0].rows, vec![("Focused node".to_string(), "none".to_string())]);
        assert_eq!(
            sections[1].rows,
            vec![("Content state".to_string(), "none".to_string())]
        );
    }

    /// The full read: node facts off graph truth, content facts off the
    /// spawn-time mirror — the same rows the pane draws and a scenario asserts.
    #[test]
    fn focused_node_with_live_content_reports_both_sections() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/page".to_string()));
        let node = app.canvas.focused_member().expect("opened node focused");
        app.apply_update(Update::ContentSpawned {
            node,
            facts: Some(ContentFacts {
                engine: "genet.web".to_string(),
                structure: Some(StructureFacts {
                    title: Some("The Page".to_string()),
                    headings: 2,
                    links: 5,
                    outline: vec![
                        crate::content::OutlineFact {
                            depth: 0,
                            role: "group",
                            name: String::new(),
                        };
                        9
                    ],
                }),
            }),
        });
        let lines = inspector_lines(&app);
        let has = |s: &str| lines.iter().any(|l| l.contains(s));
        assert!(has("URL: https://example.com/page"), "{lines:?}");
        assert!(has(&format!("Node id: {node}")));
        assert!(has("Pinned: no"));
        assert!(has("Content state: live"));
        assert!(has("Engine: genet.web"));
        assert!(has("Document title: The Page"));
        assert!(has("Document structure: headings=2 outline=9"));
        assert!(has("Outgoing links: 5"));
    }

    /// A lane without a structural read says so; nothing is synthesized.
    #[test]
    fn a_lane_without_introspection_is_reported_not_synthesized() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/x".to_string()));
        let node = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned {
            node,
            facts: Some(ContentFacts {
                engine: "some.lane".to_string(),
                structure: None,
            }),
        });
        let lines = inspector_lines(&app);
        assert!(lines.iter().any(|l| l == "Structural read: none for this lane"));
        assert!(!lines.iter().any(|l| l.starts_with("Document structure")));
    }

    /// Closing the content drops the facts with it (no stale mirror).
    #[test]
    fn closing_content_drops_the_facts() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("https://example.com/x".to_string()));
        let node = app.canvas.focused_member().unwrap();
        app.apply_update(Update::ContentSpawned {
            node,
            facts: Some(ContentFacts {
                engine: "genet.web".to_string(),
                structure: None,
            }),
        });
        assert!(app.content.facts(node).is_some());
        app.update(Action::ToggleNodeContent); // live -> close
        assert!(app.content.facts(node).is_none());
        let lines = inspector_lines(&app);
        assert!(lines.iter().any(|l| l == "Content state: none"));
        assert!(!lines.iter().any(|l| l.starts_with("Engine")));
    }
}
