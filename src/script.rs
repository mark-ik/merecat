//! Capability-scoped Piccolo control scripts.
//!
//! This is Merecat's app-control lane, not a web-page JavaScript substitute.
//! A script receives a small observation snapshot and emits ordinary
//! [`crate::action::Action`] values. The caller remains responsible for
//! lowering those actions through [`crate::app::App::update`] and running the
//! returned effects through the shell's ports.

use std::cell::RefCell;
use std::rc::Rc;

use script_engine_api::{Budget, CallCx, HostData, NativeFn, PumpOutcome, ScriptEngine};
use script_engine_piccolo::{PiccoloCallCx, PiccoloEngine};
use serde_json::json;

use crate::action::{Action, PaneKind};
use crate::app::App;

const READ_APP: u8 = 1 << 0;
const DISPATCH_ACTION: u8 = 1 << 1;
const NAVIGATE: u8 = 1 << 2;
const CONTROL_PANES: u8 = 1 << 3;

/// The capabilities a control script may exercise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ScriptCapabilities(u8);

impl ScriptCapabilities {
    pub(crate) const fn read_only() -> Self {
        Self(READ_APP)
    }

    pub(crate) const fn control() -> Self {
        Self(READ_APP | DISPATCH_ACTION | NAVIGATE | CONTROL_PANES)
    }

    const fn contains(self, capability: u8) -> bool {
        self.0 & capability != 0
    }
}

/// The read surface exposed to a control script. Keep this as data rather than
/// handing Lua an `App` reference: the script can observe the host, but it
/// cannot mutate application state except by emitting typed Actions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AppSnapshot {
    focused_url: Option<String>,
    graph_nodes: usize,
    live_content_nodes: usize,
    focus: &'static str,
    pane_count: usize,
    isometric: bool,
    height_by_degree: bool,
}

impl AppSnapshot {
    fn from_app(app: &App) -> Self {
        Self {
            focused_url: app.canvas.focused_url().map(str::to_string),
            graph_nodes: app.canvas.graph().nodes().count(),
            live_content_nodes: app.content.live_nodes().count(),
            focus: app.focus.label(),
            pane_count: app.frisket.iter_leaves().count(),
            isometric: app.canvas.is_isometric(),
            height_by_degree: app.canvas.height_by_degree(),
        }
    }

    fn to_json(&self) -> String {
        json!({
            "focused_url": self.focused_url,
            "graph_nodes": self.graph_nodes,
            "live_content_nodes": self.live_content_nodes,
            "focus": self.focus,
            "pane_count": self.pane_count,
            "isometric": self.isometric,
            "height_by_degree": self.height_by_degree,
        })
        .to_string()
    }
}

struct ControlScriptHost {
    snapshot: AppSnapshot,
    capabilities: ScriptCapabilities,
    actions: RefCell<Vec<Action>>,
}

type PiccoloValue = <PiccoloEngine as ScriptEngine>::Value;

fn host(cx: &PiccoloCallCx<'_>) -> Result<Rc<ControlScriptHost>, String> {
    cx.host_data()
        .and_then(|data| data.downcast::<ControlScriptHost>().ok())
        .ok_or_else(|| "Merecat control script has no host state".to_string())
}

fn require(host: &ControlScriptHost, capability: u8, name: &str) -> Result<(), String> {
    if host.capabilities.contains(capability) {
        Ok(())
    } else {
        Err(format!("control script lacks capability: {name}"))
    }
}

struct Snapshot;

impl NativeFn<PiccoloEngine> for Snapshot {
    fn call(cx: &mut PiccoloCallCx<'_>) -> Result<PiccoloValue, String> {
        let snapshot = {
            let host = host(cx)?;
            require(&host, READ_APP, "app.read")?;
            host.snapshot.to_json()
        };
        cx.make_string(&snapshot)
    }
}

struct Dispatch;

impl NativeFn<PiccoloEngine> for Dispatch {
    fn call(cx: &mut PiccoloCallCx<'_>) -> Result<PiccoloValue, String> {
        let value = cx.arg(0);
        let command = cx.value_to_string(&value)?;
        let action =
            action_for(&command).ok_or_else(|| format!("unknown control action: {command}"))?;
        {
            let host = host(cx)?;
            require(&host, DISPATCH_ACTION, "action.dispatch")?;
            host.actions.borrow_mut().push(action);
        }
        Ok(cx.undefined())
    }
}

struct Open;

impl NativeFn<PiccoloEngine> for Open {
    fn call(cx: &mut PiccoloCallCx<'_>) -> Result<PiccoloValue, String> {
        let value = cx.arg(0);
        let url = cx.value_to_string(&value)?;
        if url.trim().is_empty() {
            return Err("mere.open requires a non-empty address".to_string());
        }
        {
            let host = host(cx)?;
            require(&host, NAVIGATE, "navigation.open")?;
            host.actions.borrow_mut().push(Action::OpenAddress(url));
        }
        Ok(cx.undefined())
    }
}

struct Summon;

impl NativeFn<PiccoloEngine> for Summon {
    fn call(cx: &mut PiccoloCallCx<'_>) -> Result<PiccoloValue, String> {
        let value = cx.arg(0);
        let pane = cx.value_to_string(&value)?;
        let kind = pane_kind(&pane).ok_or_else(|| format!("unknown pane kind: {pane}"))?;
        {
            let host = host(cx)?;
            require(&host, CONTROL_PANES, "panes.control")?;
            host.actions.borrow_mut().push(Action::SummonPane(kind));
        }
        Ok(cx.undefined())
    }
}

fn action_for(command: &str) -> Option<Action> {
    match command.trim().to_ascii_lowercase().as_str() {
        "save_session" => Some(Action::SaveSession),
        "reseed_layout" => Some(Action::ReseedLayout),
        "toggle_isometric" => Some(Action::ToggleIsometric),
        "toggle_height_by_degree" => Some(Action::ToggleHeightByDegree),
        "toggle_live_content" => Some(Action::ToggleNodeContent),
        "close_pane" => Some(Action::CloseActivePane),
        "maximize_pane" => Some(Action::ToggleMaximizePane),
        _ => None,
    }
}

fn pane_kind(pane: &str) -> Option<PaneKind> {
    match pane.trim().to_ascii_lowercase().as_str() {
        "roster" => Some(PaneKind::Roster),
        "trail" => Some(PaneKind::Trail),
        "gloss" => Some(PaneKind::Gloss),
        "inspector" => Some(PaneKind::Inspector),
        "steward" => Some(PaneKind::Steward),
        "comms" => Some(PaneKind::Comms),
        "apparatus" => Some(PaneKind::Apparatus),
        _ => None,
    }
}

/// Run one capability-scoped Lua control script and return the Actions it
/// emitted. The host applies them later through the normal Action spine.
pub(crate) fn run(
    app: &App,
    source: &str,
    capabilities: ScriptCapabilities,
    max_steps: u64,
) -> Result<Vec<Action>, String> {
    if max_steps == 0 {
        return Err("control script requires a positive step budget".to_string());
    }

    let host = Rc::new(ControlScriptHost {
        snapshot: AppSnapshot::from_app(app),
        capabilities,
        actions: RefCell::new(Vec::new()),
    });
    let host_data: HostData = host.clone();
    let mut engine = PiccoloEngine::new().map_err(|err| format!("Piccolo init: {err:?}"))?;
    engine.set_host_data(host_data);
    engine
        .set_function::<Snapshot>("__mere_snapshot", 0)
        .map_err(|err| format!("install mere.snapshot: {err:?}"))?;
    engine
        .set_function::<Dispatch>("__mere_dispatch", 1)
        .map_err(|err| format!("install mere.dispatch: {err:?}"))?;
    engine
        .set_function::<Open>("__mere_open", 1)
        .map_err(|err| format!("install mere.open: {err:?}"))?;
    engine
        .set_function::<Summon>("__mere_summon", 1)
        .map_err(|err| format!("install mere.summon: {err:?}"))?;
    engine
        .eval(
            "mere = { snapshot = __mere_snapshot, dispatch = __mere_dispatch, \
             open = __mere_open, summon = __mere_summon }",
        )
        .map_err(|err| format!("install Merecat control API: {err:?}"))?;
    engine
        .eval_bounded(source, Budget::Steps(max_steps))
        .map_err(|err| format!("control script failed: {err:?}"))?;
    if matches!(engine.pump(Budget::Steps(max_steps)), PumpOutcome::Pending) {
        return Err("control script microtask budget exhausted".to_string());
    }

    Ok(host.actions.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_can_read_snapshot_without_mutation() {
        let app = App::test_stub();
        let actions = run(
            &app,
            "assert(type(mere.snapshot()) == 'string')",
            ScriptCapabilities::read_only(),
            200,
        )
        .unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn snapshot_json_contains_graph_and_focus_fields() {
        let snapshot = AppSnapshot::from_app(&App::test_stub()).to_json();
        assert!(snapshot.contains("\"graph_nodes\""));
        assert!(snapshot.contains("\"focus\""));
    }

    #[test]
    fn script_emits_typed_navigation_and_pane_actions() {
        let app = App::test_stub();
        let actions = run(
            &app,
            "mere.open('https://example.test'); mere.summon('roster'); mere.dispatch('save_session')",
            ScriptCapabilities::control(),
            500,
        )
        .unwrap();
        assert_eq!(
            actions,
            vec![
                Action::OpenAddress("https://example.test".into()),
                Action::SummonPane(PaneKind::Roster),
                Action::SaveSession,
            ]
        );
    }

    #[test]
    fn denied_capability_does_not_emit_an_action() {
        let app = App::test_stub();
        let err = run(
            &app,
            "mere.dispatch('save_session')",
            ScriptCapabilities::read_only(),
            200,
        )
        .unwrap_err();
        assert!(err.contains("action.dispatch"), "unexpected error: {err}");
    }

    #[test]
    fn runaway_script_hits_the_step_budget() {
        let app = App::test_stub();
        let err = run(
            &app,
            "while true do end",
            ScriptCapabilities::read_only(),
            20,
        )
        .unwrap_err();
        assert!(err.contains("budget"), "unexpected error: {err}");
    }
}
