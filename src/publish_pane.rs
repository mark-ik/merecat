//! Cambium controls for Turnstone's retained private-Knot publishing service.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, TextInput, clickable, el, lens,
    text, text_field_typed,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use layout_dom_api::LayoutDom;

use crate::publish_service::{KnotPublishingService, PublishSnapshot};

struct PublishPaneState {
    service: Option<Arc<KnotPublishingService>>,
    snapshot: PublishSnapshot,
    source_document: TextInput,
    reader: TextInput,
    hours: TextInput,
    local_status: String,
    viewport_w: f32,
    viewport_h: f32,
}

type PublishPaneView = Box<dyn AnyView<PublishPaneState, (), GenetCtx, GenetElement>>;
type PublishPaneRunner =
    GenetAppRunner<PublishPaneState, fn(&PublishPaneState) -> PublishPaneView, PublishPaneView, ()>;

fn refresh(state: &mut PublishPaneState, _: cambium::PointerClick) {
    match &state.service {
        Some(service) => {
            service.refresh();
            state.local_status = "Refreshing retained source eligibility…".into();
        }
        None => state.local_status = "Publishing needs TURNSTONE_KNOT_MODE=persona-vault.".into(),
    }
}

fn select_source(state: &mut PublishPaneState, _: cambium::PointerClick) {
    let source = state.source_document.text().trim().to_string();
    if source.is_empty() {
        state.local_status = "Enter one retained document id from the candidate list.".into();
    } else if let Some(service) = &state.service {
        service.select_source(source);
        state.local_status = "Selecting source…".into();
    } else {
        state.local_status = "Publishing needs TURNSTONE_KNOT_MODE=persona-vault.".into();
    }
}

fn issue_share(state: &mut PublishPaneState, _: cambium::PointerClick) {
    let reader = match knot::parse_hex32(state.reader.text()) {
        Ok(reader) => reader,
        Err(_) => {
            state.local_status = "Reader must be a 64-hex Personae/device public key.".into();
            return;
        }
    };
    let hours = match state.hours.text().trim().parse::<u64>() {
        Ok(hours) if hours > 0 => hours,
        _ => {
            state.local_status = "Expiry must be a positive whole number of hours.".into();
            return;
        }
    };
    let expires_at_ms = now_ms().saturating_add(hours.saturating_mul(3_600_000));
    if let Some(service) = &state.service {
        service.issue_selected(reader, Some(expires_at_ms));
        state.local_status = "Issuing reader-bound share…".into();
    } else {
        state.local_status = "Publishing needs TURNSTONE_KNOT_MODE=persona-vault.".into();
    }
}

fn revoke_latest(state: &mut PublishPaneState, _: cambium::PointerClick) {
    match (&state.service, state.snapshot.latest_share) {
        (Some(service), Some(share)) => {
            service.revoke(share);
            state.local_status = "Revoking the latest share…".into();
        }
        (_, None) => state.local_status = "No retained share is selected for revocation.".into(),
        (None, _) => {
            state.local_status = "Publishing needs TURNSTONE_KNOT_MODE=persona-vault.".into()
        }
    }
}

fn withdraw_selected(state: &mut PublishPaneState, _: cambium::PointerClick) {
    match (&state.service, state.snapshot.selected) {
        (Some(service), Some(publication)) => {
            service.unpublish(publication);
            state.local_status = "Withdrawing publication…".into();
        }
        (_, None) => state.local_status = "Select a publication before withdrawing it.".into(),
        (None, _) => {
            state.local_status = "Publishing needs TURNSTONE_KNOT_MODE=persona-vault.".into()
        }
    }
}

fn field(
    getter: fn(&mut PublishPaneState) -> &mut TextInput,
    label: &'static str,
) -> PublishPaneView {
    let input = Box::new(lens(
        move |input: &mut TextInput| text_field_typed(input),
        getter,
    )) as PublishPaneView;
    Box::new(
        el::<_, PublishPaneState, ()>("div", (el::<_, PublishPaneState, ()>("div", label), input))
            .attr("class", "setting-row"),
    )
}

fn publish_pane_view(state: &PublishPaneState) -> PublishPaneView {
    let candidate_rows: Vec<PublishPaneView> = state
        .snapshot
        .candidates
        .iter()
        .map(|candidate| {
            let selected = candidate
                .publication
                .map(|id| format!(" selected {}", id.as_uuid()))
                .unwrap_or_default();
            let head = candidate
                .head
                .map(|head| knot::hex32(&head)[..8].to_string())
                .unwrap_or_else(|| "none".into());
            Box::new(
                el::<_, PublishPaneState, ()>(
                    "div",
                    format!(
                        "{} · {} · {:?} · {} · head {head}{selected}",
                        candidate.title,
                        candidate.source_document,
                        candidate.eligibility,
                        candidate.media_type,
                    ),
                )
                .attr("class", "list-row"),
            ) as PublishPaneView
        })
        .collect();
    let share_rows: Vec<PublishPaneView> = state
        .snapshot
        .shares
        .iter()
        .map(|share| {
            let state = if share.revoked { "revoked" } else { "active" };
            Box::new(
                el::<_, PublishPaneState, ()>(
                    "div",
                    format!(
                        "share {} · {} · recipient {} · {:?}",
                        share.id,
                        state,
                        &knot::hex32(&share.reader)[..8],
                        share.expires_at_ms,
                    ),
                )
                .attr("class", "list-row"),
            ) as PublishPaneView
        })
        .collect();
    let controls = (
        Box::new(clickable(
            el::<_, PublishPaneState, ()>("button", text("Refresh sources"))
                .attr("class", "setting-apply"),
            refresh,
        )) as PublishPaneView,
        Box::new(clickable(
            el::<_, PublishPaneState, ()>("button", text("Select source"))
                .attr("class", "setting-apply"),
            select_source,
        )) as PublishPaneView,
        Box::new(clickable(
            el::<_, PublishPaneState, ()>("button", text("Issue share"))
                .attr("class", "setting-apply"),
            issue_share,
        )) as PublishPaneView,
        Box::new(clickable(
            el::<_, PublishPaneState, ()>("button", text("Revoke latest share"))
                .attr("class", "setting-apply"),
            revoke_latest,
        )) as PublishPaneView,
        Box::new(clickable(
            el::<_, PublishPaneState, ()>("button", text("Withdraw selected"))
                .attr("class", "setting-apply"),
            withdraw_selected,
        )) as PublishPaneView,
    );
    let status = if state.local_status.is_empty() {
        state.snapshot.status.clone()
    } else {
        state.local_status.clone()
    };
    let ticket = state
        .snapshot
        .latest_ticket
        .as_deref()
        .map(|ticket| format!("Latest private handoff ticket: {ticket}"))
        .unwrap_or_else(|| "No ticket has been issued in this service lifetime.".into());
    Box::new(
        el::<_, PublishPaneState, ()>(
            "div",
            (
                el::<_, PublishPaneState, ()>("div", "Private Knot publishing")
                    .attr("class", "list-section-title"),
                el::<_, PublishPaneState, ()>("div", state.snapshot.route.clone())
                    .attr("class", "list-row muted"),
                el::<_, PublishPaneState, ()>("div", "Eligible retained sources")
                    .attr("class", "list-section-title"),
                el::<_, PublishPaneState, ()>("div", candidate_rows),
                field(|state| &mut state.source_document, "Source document id"),
                field(|state| &mut state.reader, "Reader public key"),
                field(|state| &mut state.hours, "Share expiry hours"),
                el::<_, PublishPaneState, ()>("div", controls).attr("class", "setting-row"),
                el::<_, PublishPaneState, ()>("div", "Issued shares")
                    .attr("class", "list-section-title"),
                el::<_, PublishPaneState, ()>("div", share_rows),
                el::<_, PublishPaneState, ()>("div", ticket).attr("class", "list-row muted"),
                el::<_, PublishPaneState, ()>("div", status).attr("class", "list-row muted"),
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

/// A retained publishing control panel. It remains useful while unavailable:
/// the status says exactly which persona-vault service needs configuring.
pub struct PublishPane {
    dom: DomHandle,
    runner: PublishPaneRunner,
}

impl PublishPane {
    pub fn new(service: Option<Arc<KnotPublishingService>>) -> Self {
        let default_hours = KnotPublishingService::default_share_hours().to_string();
        let dom: DomHandle = Rc::new(RefCell::new(ScriptedDom::new()));
        let state = PublishPaneState {
            service,
            snapshot: PublishSnapshot::default(),
            source_document: TextInput::new(String::new()),
            reader: TextInput::new(String::new()),
            hours: TextInput::new(default_hours),
            local_status: String::new(),
            viewport_w: 0.0,
            viewport_h: 0.0,
        };
        let runner = PublishPaneRunner::new(
            dom.clone(),
            publish_pane_view as fn(&PublishPaneState) -> PublishPaneView,
            state,
        );
        Self { dom, runner }
    }

    pub fn sync(&mut self, pane_w: f32, pane_h: f32) {
        self.runner.update(|state| {
            if let Some(service) = &state.service {
                state.snapshot = service.snapshot();
                state.local_status.clear();
            }
            state.viewport_w = pane_w;
            state.viewport_h = pane_h;
        });
    }

    pub fn scene(&self, w: u32, h: u32) -> netrender::Scene {
        crate::ui::scene_from_dom(&self.dom.borrow(), crate::ui::CAMBIUM_SHEET, w, h)
    }

    pub fn click(&mut self, x: f32, y: f32, w: u32, h: u32) {
        let hit = {
            let dom = self.dom.borrow();
            let layout =
                IncrementalLayout::new(&*dom, &[crate::ui::CAMBIUM_SHEET], w as f32, h as f32);
            layout.hit_test(&*dom, x, y, &ScrollOffsets::<NodeId>::default())
        };
        if let Some(node) = hit {
            let _: Vec<()> = self
                .runner
                .dispatch_click(node, cambium::PointerClick::at((x, y)));
        }
    }

    pub fn dom_ref(&self) -> std::cell::Ref<'_, ScriptedDom> {
        self.dom.borrow()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_panel_is_an_honest_configured_surface() {
        let mut pane = PublishPane::new(None);
        pane.sync(400.0, 600.0);
        let dom = pane.dom_ref();
        assert_eq!(dom.all_with_class(dom.document(), "setting-apply").len(), 5);
        assert!(
            dom.all_with_class(dom.document(), "list-section-title")
                .into_iter()
                .filter_map(|node| dom.text(node))
                .any(|text| text.contains("Knot publishing"))
        );
    }
}
