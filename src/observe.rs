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
