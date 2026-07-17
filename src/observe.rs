//! The observation surface: one snapshot + one event stream over app truth
//! (the architecture plan's recorded snapshot/events pair, landed at its
//! trigger — the scenario lane is the first automation consumer, and its
//! asserts read THIS surface instead of poking app fields one by one). The
//! same surface is what the a11y projection, diagnostics, and the
//! session-engines plan's automation story consume later: observation is
//! the vocabulary's other half, so it lives beside `action`, app-owned and
//! port-agnostic.
//!
//! Scope note: events are emitted where Actions and Updates fold — the
//! semantic tier. Continuous gestures bypass Action by the gesture law, so
//! a gesture-end semantic change (click-selection, drag-placement) does not
//! yet emit; that arrives with the gesture-end events the law already
//! promises, not by teaching this module about pointers.

use uuid::Uuid;

use crate::app::App;
use crate::content::NodeContent;
use crate::ui::Suggestion;

/// One coherent read of the application's observable state.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// The focused node, when exactly one is selected.
    pub focused: Option<FocusedNode>,
    pub omnibar: OmnibarView,
    /// Per-node content lifecycle, as (member, state label) pairs.
    pub content: Vec<(Uuid, String)>,
    pub node_count: usize,
    /// Whether at least one node lies inside the viewport.
    pub graph_visible: bool,
    /// The composited surfaces present this frame, as kind labels in z-order
    /// (rung 5 slice A). Derived from app truth: canvas is always present,
    /// content when the focused node is live, chrome when it has content.
    /// The window size lives in the shell, so this is the surface LIST, not
    /// pixel rects.
    pub surfaces: Vec<String>,
    /// Which surface holds semantic input, as a label ("canvas" / "chrome" /
    /// "content").
    pub focus: String,
    /// The panes in the frisket tree, as `PaneContent` tags (rung 5 slice C).
    /// A single-pane layout reads `["orrery"]`; summoning a Roster adds
    /// `"roster"`. The active pane, if any, is `active_pane`.
    pub panes: Vec<String>,
    /// The active pane's tag, or `None` when the canvas (Orrery) is active.
    pub active_pane: Option<String>,
    /// Whether a pane is maximized.
    pub maximized: bool,
    /// When a Trail pane is in the tree, its row texts (rung 5 slice D), so a
    /// scenario can assert a row's content. Empty when no Trail pane is open.
    pub trail_rows: Vec<String>,
    /// When a Roster pane is in the tree, its row texts (the node manifest).
    pub roster_rows: Vec<String>,
    /// The Roster's active tab label, mirrored out of the strip's own state.
    pub roster_tab: &'static str,
    /// The root split's ratio, when the pane tree is split at all. The divider
    /// receipts assert against this after a drag.
    pub split_ratio: Option<f32>,
}

/// The focused node's identity and captions, as the UI would present them.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusedNode {
    pub member: Uuid,
    pub url: String,
    /// The at-rest caption (display label, plus host when it adds info).
    pub caption: String,
}

/// The omnibar as observed: state plus suggestion rows as display strings.
#[derive(Clone, Debug, PartialEq)]
pub struct OmnibarView {
    pub open: bool,
    pub text: String,
    pub cursor: usize,
    pub selected: usize,
    pub suggestions: Vec<String>,
}

/// A semantic event: something durable or externally observable happened.
/// Drained by the shell each frame (into the scenario's log, or dropped);
/// later consumers (diagnostics, automation) subscribe at the same drain.
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    AddressOpened(String),
    OmnibarOpened,
    OmnibarClosed,
    /// A commit resolved to a suggestion (its display string).
    OmnibarCommitted(String),
    LayoutReseeded,
    ContentState { node: Uuid, state: String },
    /// A pane of the named kind was summoned into the tree (rung 5 slice C).
    PaneSummoned(&'static str),
    /// The active pane was closed.
    PaneClosed,
    /// A pane interaction named a target that is not on screen — a
    /// `click-row`/`click-tab`/`click-node` that resolved to nothing. Divergence
    /// a driving script or model must be able to see: the aim missed, and a
    /// receipt that only checks the end state would call the miss a pass. `what`
    /// is the interaction kind, `target` the name that did not resolve.
    InteractionMissed { what: &'static str, target: String },
    /// An affordance fired that is not wired yet — today only Trail's Recover,
    /// which awaits the deletion log (rung 6). A known-not-yet state, emitted so
    /// a scenario asserts the gap explicitly rather than a silent no-op.
    AffordanceUnavailable { what: &'static str, target: String },
}

impl AppEvent {
    /// A grep-friendly one-line rendering (what `assert event` matches).
    pub fn describe(&self) -> String {
        match self {
            AppEvent::AddressOpened(url) => format!("address-opened {url}"),
            AppEvent::OmnibarOpened => "omnibar-opened".to_string(),
            AppEvent::OmnibarClosed => "omnibar-closed".to_string(),
            AppEvent::OmnibarCommitted(what) => format!("omnibar-committed {what}"),
            AppEvent::LayoutReseeded => "layout-reseeded".to_string(),
            AppEvent::ContentState { node, state } => format!("content {node} {state}"),
            AppEvent::PaneSummoned(kind) => format!("pane-summoned {kind}"),
            AppEvent::PaneClosed => "pane-closed".to_string(),
            AppEvent::InteractionMissed { what, target } => {
                format!("interaction-missed {what} {target}")
            }
            AppEvent::AffordanceUnavailable { what, target } => {
                format!("affordance-unavailable {what} {target}")
            }
        }
    }
}

/// Read the application snapshot. Pure; the app is not disturbed.
pub fn snapshot(app: &App) -> Snapshot {
    let focused = app.canvas.focused_member().and_then(|member| {
        let url = app.canvas.focused_url()?.to_string();
        let caption = crate::app::focused_caption(&app.canvas)?;
        Some(FocusedNode {
            member,
            url,
            caption,
        })
    });
    let content = app
        .canvas
        .graph()
        .nodes()
        .filter_map(|(_, n)| {
            let state = match app.content.get(n.id)? {
                NodeContent::Requested => "requested".to_string(),
                NodeContent::Live => "live".to_string(),
                NodeContent::Failed(err) => format!("failed: {err}"),
            };
            Some((n.id, state))
        })
        .collect();
    // The surface list, derived from app truth (the shell owns the live sessions
    // and the window size; observe reports what a frame would compose). The base
    // is the frisket tree — the Orrery leaf is the canvas, every other leaf a
    // pane — then content over the canvas when the focused node is Live, then
    // chrome on top when it has something to show.
    let mut surfaces: Vec<String> = app
        .frisket
        .iter_leaves()
        .map(|(_, content, _)| {
            if matches!(content, frisket::PaneContent::Orrery) {
                "canvas".to_string()
            } else {
                "pane".to_string()
            }
        })
        .collect();
    // A split tree has seams: one divider surface per split node.
    if matches!(app.frisket.root, frisket::PaneNode::Split { .. }) && app.maximized.is_none() {
        surfaces.push("divider".to_string());
    }
    let focused_live = app
        .canvas
        .focused_member()
        .is_some_and(|m| matches!(app.content.get(m), Some(NodeContent::Live)));
    if focused_live {
        surfaces.push("content".to_string());
    }
    if crate::ui::chrome_has_content(&app.omnibar, crate::app::focused_caption(&app.canvas).as_deref())
    {
        surfaces.push("chrome".to_string());
    }
    Snapshot {
        focused,
        omnibar: OmnibarView {
            open: app.omnibar.open,
            text: app.omnibar.text.clone(),
            cursor: app.omnibar.cursor,
            selected: app.omnibar.selected,
            suggestions: app.omnibar.suggestions.iter().map(suggestion_line).collect(),
        },
        content,
        node_count: app.canvas.graph().nodes().count(),
        graph_visible: app.canvas.graph_visible(),
        surfaces,
        focus: app.focus.label().to_string(),
        panes: app
            .frisket
            .iter_leaves()
            .map(|(_, content, _)| content.tag().to_string())
            .collect(),
        active_pane: app.active_pane.and_then(|id| {
            app.frisket
                .iter_leaves()
                .find(|(pid, _, _)| *pid == id)
                .map(|(_, content, _)| content.tag().to_string())
        }),
        maximized: app.maximized.is_some(),
        trail_rows: app
            .frisket
            .iter_leaves()
            .any(|(_, c, _)| matches!(c, frisket::PaneContent::Trail))
            .then(|| {
                crate::trail_view::trail_rows(app)
                    .into_iter()
                    .map(|r| r.text)
                    .collect()
            })
            .unwrap_or_default(),
        roster_rows: app
            .frisket
            .iter_leaves()
            .any(|(_, c, _)| matches!(c, frisket::PaneContent::Roster))
            .then(|| {
                crate::roster_view::roster_rows(app)
                    .into_iter()
                    .map(|r| r.text)
                    .collect()
            })
            .unwrap_or_default(),
        roster_tab: crate::cambium_pane::tab_label(app.roster_tab),
        split_ratio: match &app.frisket.root {
            frisket::PaneNode::Split { ratio, .. } => Some(*ratio),
            frisket::PaneNode::Leaf { .. } => None,
        },
    }
}

/// One suggestion row as its display string (the assert/a11y rendering).
pub fn suggestion_line(s: &Suggestion) -> String {
    match s {
        Suggestion::Node { label, host, .. } if !host.is_empty() => format!("{label} \u{00b7} {host}"),
        Suggestion::Node { label, .. } => label.clone(),
        Suggestion::Go { url } => format!("go {url}"),
        Suggestion::Act { label, .. } => format!("\u{203a} {label}"),
        Suggestion::Hint(h) => h.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;

    #[test]
    fn snapshot_reads_focus_omnibar_and_content_coherently() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        app.update(Action::OmnibarOpen { command: true });
        app.update(Action::OmnibarChar('r'));

        let snap = snapshot(&app);
        let focused = snap.focused.expect("the opened node is focused");
        assert_eq!(focused.url, "mere://alpha");
        assert!(snap.omnibar.open);
        assert_eq!(snap.omnibar.text, ">r");
        assert!(
            snap.omnibar.suggestions.iter().any(|s| s.contains("Reseed layout")),
            "suggestion rows render as display strings: {:?}",
            snap.omnibar.suggestions
        );
        assert_eq!(snap.node_count, 1);
        assert!(snap.content.is_empty(), "no content lifecycle yet");
    }

    #[test]
    fn semantic_actions_emit_events() {
        let mut app = App::test_stub();
        app.update(Action::OpenAddress("mere://alpha".to_string()));
        app.update(Action::OmnibarOpen { command: false });
        app.update(Action::OmnibarClose);
        app.update(Action::ToggleNodeContent);
        let described: Vec<String> = app.take_events().iter().map(AppEvent::describe).collect();
        assert!(described.iter().any(|e| e == "address-opened mere://alpha"));
        assert!(described.iter().any(|e| e == "omnibar-opened"));
        assert!(described.iter().any(|e| e == "omnibar-closed"));
        assert!(
            described.iter().any(|e| e.starts_with("content ") && e.ends_with(" requested")),
            "{described:?}"
        );
        assert!(app.take_events().is_empty(), "take drains");
    }
}
