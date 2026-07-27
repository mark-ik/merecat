//! The action catalog: what the app offers right now, and in what order.
//!
//! Contextual rows lead the static registry, because a pending grant review
//! must be the first thing an opened palette shows. One composition, read by
//! the `>` lane, the snapshot, and the automation runner alike — composing it
//! twice is how they come to disagree about what a label means.

use crate::action::{Action, Effect, PaneKind};
use crate::observe::AppEvent;
use crate::panes::PaneContent;

use super::{App, pane_label};

impl App {
    /// The dynamic switcher entries for the omnibar's `>` lane: a switch per
    /// OTHER session, most recently updated first ("New session" is a static
    /// palette entry).
    /// The denizen rows for the palette's actions lane: the pending
    /// install's visible review (the Confirm row IS the ask), then one Run
    /// row per resident (B1: the palette populated from denizen residency).
    /// Lower a denizen's emitted Actions through this same spine with the
    /// journal scoped to its subject, so every captured graph edit reads back
    /// attributed. Shared by both runnable lanes: piccolo returns Actions
    /// after evaluation, the component lane returns the ring-gate's accepted
    /// queue — by here, both are authorized.
    pub(super) fn lower_denizen_actions(
        &mut self,
        subject: servitor::Subject,
        label: String,
        actions: Vec<Action>,
    ) -> Vec<Effect> {
        if let Ok(mut journal) = self.journal.lock() {
            journal.set_author(subject.to_hex());
        }
        let mut effects = Vec::new();
        for action in actions {
            effects.extend(self.update(action));
        }
        if let Ok(mut journal) = self.journal.lock() {
            journal.set_author(mere::kernel::graph::USER_AUTHOR);
        }
        self.events.push(AppEvent::DenizenRan(label));
        effects.push(Effect::SaveSession);
        effects.push(Effect::Redraw);
        effects
    }

    pub fn denizen_actions(&self) -> Vec<(String, Action)> {
        let mut rows = Vec::new();
        if let Some(pending) = &self.pending_install {
            rows.push((
                crate::denizen::review_line(pending),
                Action::ConfirmInstallDenizen,
            ));
            rows.push((
                format!("Cancel install {}", pending.label),
                Action::CancelInstallDenizen,
            ));
        }
        let mut residents: Vec<_> = self.denizens.residents.iter().collect();
        residents.sort_by(|(_, a), (_, b)| a.label.cmp(&b.label));
        for (member, resident) in residents {
            rows.push((
                format!("Run {}", resident.label),
                Action::RunDenizen { member: *member },
            ));
            rows.push((
                format!("Uninstall {}", resident.label),
                Action::UninstallDenizen { member: *member },
            ));
        }
        rows
    }

    pub fn session_actions(&self) -> Vec<(String, Action)> {
        // Denizen rows lead: a pending install's review must be the first
        // thing the opened palette shows (B1's visible grant review).
        let mut rows = self.denizen_actions();
        let mut others: Vec<_> = self
            .sessions
            .iter()
            .filter(|(id, _)| *id != self.session_id)
            .collect();
        others.sort_by_key(|(_, m)| std::cmp::Reverse(m.updated_at));
        rows.extend(others.into_iter().map(|(id, _)| {
            (
                format!("Switch to session {}", self.session_label(id)),
                Action::SwitchSession(id),
            )
        }));
        rows.extend(self.pane_section_actions());
        rows
    }

    /// **The** action catalog offered right now: the contextual rows LEAD the
    /// static registry, because a pending denizen install's grant review must be
    /// the first thing an opened palette shows (participant gate B1) and the
    /// contextual rows outrank the fixed verbs generally.
    ///
    /// One composition, read by everything that offers or resolves an action:
    /// the omnibar's `>` lane filters it, the observation snapshot reports it,
    /// and the automation runner resolves a label through it. Composing it in
    /// more than one place is how the runner and the palette come to disagree
    /// about what a label means (they did: the runner resolved static-first
    /// while the palette showed dynamic-first, so a dynamic row that shadowed a
    /// static label would have acted as the wrong one).
    pub fn available_actions(&self) -> Vec<(String, Action)> {
        let mut rows = self.session_actions();
        rows.extend(
            crate::action::palette_actions()
                .into_iter()
                .map(|(label, action)| (label.to_string(), action)),
        );
        rows
    }

    /// The composed-section rows for the ACTIVE pane, when its content composes
    /// (a Gloss, an Overmap): one add/remove per registered provider, plus the
    /// reorder rows. Pane-scoped palette entries are how the gloss-composite
    /// design chose to expose composition (the right-click palette already
    /// selects the pane under the pointer), so no new chrome. Empty when the
    /// active pane is not a composable one.
    ///
    /// Written against `PaneContent::composition`, not a pane kind, so a pane
    /// that gains a composition gains this whole UI without touching it. The
    /// row's prefix is the pane's own tag, so it names itself too.
    fn pane_section_actions(&self) -> Vec<(String, Action)> {
        let Some(pane) = self.active_pane else {
            return Vec::new();
        };
        let Some(content) = self.pane_content(pane) else {
            return Vec::new();
        };
        let Some(cfg) = content.composition() else {
            return Vec::new();
        };
        let who = pane_label(content);
        let mut rows: Vec<(String, Action)> = crate::sections::ALL
            .iter()
            .map(|p| {
                let on = cfg.sections.iter().any(|id| id == p.id);
                let verb = if on { "remove" } else { "add" };
                (
                    format!("{who}: {verb} section — {}", p.title),
                    Action::TogglePaneSection {
                        pane,
                        section: p.id.to_string(),
                    },
                )
            })
            .collect();
        // Reorder rows only where a move would DO something: nothing to
        // reorder with one section, and no "up" on the first (the palette
        // should not offer a no-op).
        if cfg.sections.len() > 1 {
            for (i, id) in cfg.sections.iter().enumerate() {
                let Some(p) = crate::sections::by_id(id) else {
                    continue;
                };
                if i > 0 {
                    rows.push((
                        format!("{who}: move section up — {}", p.title),
                        Action::MovePaneSection {
                            pane,
                            section: id.clone(),
                            delta: -1,
                        },
                    ));
                }
                if i + 1 < cfg.sections.len() {
                    rows.push((
                        format!("{who}: move section down — {}", p.title),
                        Action::MovePaneSection {
                            pane,
                            section: id.clone(),
                            delta: 1,
                        },
                    ));
                }
            }
        }
        rows
    }
}
