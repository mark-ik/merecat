//! The application-settings projection.
//!
//! This is the first host UI consumer of `genet_host_api::settings`. The pane
//! owns only retained control state and the Turnstone provider; persistence
//! still belongs to `session-runtime` through that provider.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, Slider, TextInput, clickable, el,
    lens, slider, text, text_field_typed,
};
use genet_host_api::settings::{
    SettingControl, SettingSpec, SettingValue, SettingsProjection, SettingsProvider,
};
use genet_host_api::tile::SettingsRef;
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::LayoutDom;

use crate::settings_provider::{APPEARANCE_REFERENCE, ApplicationSettingsProvider};

struct SettingsState {
    provider: ApplicationSettingsProvider,
    theme_id: TextInput,
    theme_mode: TextInput,
    zoom: Slider,
    status: String,
    viewport_w: f32,
    viewport_h: f32,
}

type SettingsView = Box<dyn AnyView<SettingsState, (), GenetCtx, GenetElement>>;
type SettingsRunner =
    GenetAppRunner<SettingsState, fn(&SettingsState) -> SettingsView, SettingsView, ()>;

fn apply_text(setting_id: &'static str) -> impl Fn(&mut SettingsState, cambium::PointerClick) {
    move |state, _| {
        let value = match setting_id {
            "theme.id" => state.theme_id.text().to_owned(),
            "theme.mode" => state.theme_mode.text().to_owned(),
            _ => return,
        };
        apply_value(state, setting_id, SettingValue::Text(value));
    }
}

fn apply_zoom(state: &mut SettingsState, _: cambium::PointerClick) {
    let value = 0.5 + f64::from(state.zoom.value.clamp(0.0, 1.0)) * 2.5;
    apply_value(state, "ui.zoom", SettingValue::Number(value));
}

fn apply_value(state: &mut SettingsState, setting_id: &str, value: SettingValue) {
    let reference = SettingsRef(APPEARANCE_REFERENCE.into());
    state.status = match state.provider.apply(&reference, setting_id, value) {
        Ok(()) => format!("Saved {setting_id}"),
        Err(error) => format!("Could not save {setting_id}: {error:?}"),
    };
}

fn setting_row(spec: &SettingSpec) -> SettingsView {
    let label = el::<_, SettingsState, ()>(
        "div",
        format!("{} · {:?} · {:?}", spec.label, spec.scope, spec.movement),
    )
    .attr("class", "setting-label");

    match (&spec.control, spec.id.as_str()) {
        (SettingControl::Text, "theme.id") => {
            let field = Box::new(lens(
                |input: &mut TextInput| text_field_typed(input),
                |state: &mut SettingsState| &mut state.theme_id,
            )) as SettingsView;
            let apply = Box::new(clickable(
                el::<_, SettingsState, ()>("button", text("Apply"))
                    .attr("class", "setting-apply")
                    .attr("data-setting", "theme.id"),
                apply_text("theme.id"),
            )) as SettingsView;
            Box::new(
                el::<_, SettingsState, ()>("div", (label, field, apply))
                    .attr("class", "setting-row"),
            )
        }
        (SettingControl::Text, "theme.mode") => {
            let field = Box::new(lens(
                |input: &mut TextInput| text_field_typed(input),
                |state: &mut SettingsState| &mut state.theme_mode,
            )) as SettingsView;
            let apply = Box::new(clickable(
                el::<_, SettingsState, ()>("button", text("Apply"))
                    .attr("class", "setting-apply")
                    .attr("data-setting", "theme.mode"),
                apply_text("theme.mode"),
            )) as SettingsView;
            Box::new(
                el::<_, SettingsState, ()>("div", (label, field, apply))
                    .attr("class", "setting-row"),
            )
        }
        (SettingControl::Number { .. }, "ui.zoom") => {
            let control = Box::new(lens(
                |control: &mut Slider| slider(control),
                |state: &mut SettingsState| &mut state.zoom,
            )) as SettingsView;
            let apply = Box::new(clickable(
                el::<_, SettingsState, ()>("button", text("Apply"))
                    .attr("class", "setting-apply")
                    .attr("data-setting", "ui.zoom"),
                apply_zoom,
            )) as SettingsView;
            Box::new(
                el::<_, SettingsState, ()>("div", (label, control, apply))
                    .attr("class", "setting-row"),
            )
        }
        _ => Box::new(
            el::<_, SettingsState, ()>(
                "div",
                (
                    label,
                    el::<_, SettingsState, ()>("div", "unsupported control"),
                ),
            )
            .attr("class", "setting-row setting-unsupported"),
        ),
    }
}

fn settings_view(state: &SettingsState) -> SettingsView {
    let reference = SettingsRef(APPEARANCE_REFERENCE.into());
    let projection = SettingsProjection::resolve(&state.provider, &reference);
    let rows: Vec<SettingsView> = projection
        .map(|projection| projection.specs)
        .unwrap_or_default()
        .iter()
        .map(setting_row)
        .collect();
    Box::new(
        el::<_, SettingsState, ()>(
            "div",
            (
                el::<_, SettingsState, ()>("div", "Application settings")
                    .attr("class", "list-section-title"),
                el::<_, SettingsState, ()>("div", "pelt/appearance")
                    .attr("class", "list-row muted"),
                el::<_, SettingsState, ()>("div", rows),
                el::<_, SettingsState, ()>("div", state.status.clone())
                    .attr("class", "list-row muted"),
            ),
        )
        .attr("class", "pane")
        .attr(
            "style",
            format!(
                "width: {}px; height: {}px;",
                state.viewport_w, state.viewport_h
            ),
        ),
    )
}

/// A retained application-settings projection over Turnstone's provider.
pub struct SettingsPane {
    dom: DomHandle,
    runner: SettingsRunner,
}

impl SettingsPane {
    pub fn new(data_root: PathBuf) -> Self {
        let (provider, status) = match ApplicationSettingsProvider::load(&data_root) {
            Ok(provider) => (provider, String::new()),
            Err(error) => (
                ApplicationSettingsProvider::from_settings(
                    data_root,
                    session_runtime::ApplicationSettings::default(),
                ),
                format!("Could not load application settings: {error}"),
            ),
        };
        let theme_id = provider.settings().theme_id.clone().unwrap_or_default();
        let theme_mode = provider.settings().theme_mode.clone().unwrap_or_default();
        let ui_zoom = provider.settings().ui_zoom;
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = SettingsState {
            provider,
            theme_id: TextInput::new(theme_id),
            theme_mode: TextInput::new(theme_mode),
            zoom: Slider::new(((ui_zoom - 0.5) / 2.5).clamp(0.0, 1.0))
                .with_label("UI zoom")
                .with_steps(0.02, 0.1),
            status,
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = SettingsRunner::new(
            dom.clone(),
            settings_view as fn(&SettingsState) -> SettingsView,
            state,
        );
        Self { dom, runner }
    }

    pub fn sync(&mut self, pane_w: f32, pane_h: f32) {
        self.runner.update(|state| {
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    pub fn dom_ref(&self) -> std::cell::Ref<'_, ScriptedDom> {
        self.dom.borrow()
    }

    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            let scroll = ScrollOffsets::<NodeId>::default();
            layout.hit_test(&*dom, x, y, &scroll)
        };
        if let Some(node) = hit {
            let _: Vec<()> = self
                .runner
                .dispatch_click(node, cambium::PointerClick::at((x, y)));
        }
    }

    pub fn settings(&self) -> &session_runtime::ApplicationSettings {
        self.runner.state().provider.settings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "turnstone-settings-pane-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn pane_renders_provider_described_controls() {
        let pane = SettingsPane::new(root("render"));
        let dom = pane.dom_ref();
        assert_eq!(dom.all_with_class(dom.document(), "setting-row").len(), 3);
        assert_eq!(dom.all_with_class(dom.document(), "setting-label").len(), 3);
        assert_eq!(dom.all_with_class(dom.document(), "setting-apply").len(), 3);
        assert_eq!(dom.all_with_class(dom.document(), "slider-track").len(), 1);
    }

    #[test]
    fn provider_owner_is_available_to_host_after_projection_construction() {
        let pane = SettingsPane::new(root("owner"));
        assert_eq!(pane.settings().ui_zoom, 1.1);
        assert_eq!(pane.settings().theme_id, None);
    }
}
