//! The action-ring classifier: the envelope lane's permission model.
//!
//! A wasm (or any future) denizen emits actions through ONE stable envelope
//! (`mere:script`'s `actions` interface — `{name, payload}`), and the whole
//! `Action` surface is potentially emittable: no curated interface, no second
//! compile-time authority. What decides is the action's **ring** — a
//! capability-path family the emission classifies into — checked against the
//! denizen's grant at the moment of emission, exactly where the piccolo lane
//! already denies (B2: capability from the grant, not a feature flag).
//!
//! Rings, mapped to grantable paths:
//! - navigate (`app/navigate`) — moving through content
//! - panes (`app/panes`) — window / pane / workbench arrangement
//! - dispatch (`app/dispatch`) — node + view edits, and the omnibar (the
//!   command surface)
//! - session (`app/session`) — fork / switch / close / delete / recover
//! - **host-only** — NO grantable path exists. Gate management
//!   (install / confirm / cancel / run) can never be covered by any
//!   authority: a component confirming its own grant review would be
//!   self-escalation, so it is structurally impossible, not policy-denied.
//!
//! [`ring_of`] is an exhaustive match with NO catch-all: adding an `Action`
//! variant without classifying it is a compile error, never a silent default.
//! A default profile ("scenario packs come preselected with navigate")
//! shapes the install review's checkboxes; it never grants silently — the
//! visible review stays the only place an ask becomes a grant.

use servitor::{Mode, PrefixAuthority, Subject};

use crate::action::Action;

/// The permission ring an action classifies into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    Navigate,
    Panes,
    Dispatch,
    Session,
    HostOnly,
}

impl Ring {
    /// The grantable capability path this ring checks under (`Mode::Write`),
    /// or `None` for the host-only ring — the structural floor: no path
    /// exists, so no grant can ever cover it.
    pub fn path(self) -> Option<&'static str> {
        match self {
            Ring::Navigate => Some("app/navigate"),
            Ring::Panes => Some("app/panes"),
            Ring::Dispatch => Some("app/dispatch"),
            Ring::Session => Some("app/session"),
            Ring::HostOnly => None,
        }
    }

    /// The ring's display name (denials name the ring, attributably).
    pub fn name(self) -> &'static str {
        match self {
            Ring::Navigate => "navigate",
            Ring::Panes => "panes",
            Ring::Dispatch => "dispatch",
            Ring::Session => "session",
            Ring::HostOnly => "host-only",
        }
    }
}

/// Classify an action into its ring. Exhaustive on purpose: a new `Action`
/// variant fails to compile until someone decides its ring.
pub fn ring_of(action: &Action) -> Ring {
    use Action::*;
    match action {
        // Moving through content.
        OpenAddress(_) | NavBack | NavForward | Reload => Ring::Navigate,

        // Window / pane / workbench arrangement.
        NewWindow
        | TearOutActivePane
        | SummonPane(_)
        | CloseActivePane
        | SetActivePaneDivider(_)
        | SetSplitRatio { .. }
        | ToggleMaximizePane
        // Composing a pane's list sections edits its LEAF (the layout), so it
        // is arrangement, not a node/view edit.
        | TogglePaneSection { .. }
        | OpenInWorkbench
        | TearOutTile { .. }
        | WorkbenchActivate(_)
        | CloseWorkbenchTile
        | WorkbenchStackOnto { .. }
        | WorkbenchSplitBeside { .. }
        | WorkbenchSplitOut { .. }
        | WorkbenchSetFractions { .. } => Ring::Panes,

        // Node + view edits, and the omnibar: driving the command surface IS
        // dispatch (an omnibar commit can do anything a suggestion offers).
        SetNodeSprite { .. }
        | SetViewerOverride { .. }
        | ReseedLayout
        | FitView
        | SetLayoutStrategy(_)
        | ToggleIsometric
        | OrbitBy(_)
        | TiltBy(_)
        | ToggleHeightByDegree
        | TogglePhysics
        | ToggleSizeByRecency
        | ToggleNodeContent
        | OmnibarOpen { .. }
        | OmnibarClose
        | OmnibarChar(_)
        | OmnibarInsert(_)
        | OmnibarBackspace
        | OmnibarDelete
        | OmnibarCaret(_)
        | OmnibarMove(_)
        | OmnibarCommit
        | OmnibarCommitRow(_) => Ring::Dispatch,

        // The session tier: whole-session lifecycle and the recycle bin.
        SaveSession
        | ForkNode { .. }
        | ForkFocusedNode
        | NewSession
        | SwitchSession(_)
        | CloseSession
        | BeginRenameSession
        | RenameSession { .. }
        | DeleteFocusedNode
        | RecoverDeletedNode(_)
        | EmptyRecycleBin
        | RecoverSession(_) => Ring::Session,

        // Gate management: never emittable in effect, whatever the grant.
        InstallDenizen { .. } | ConfirmInstallDenizen | CancelInstallDenizen
        | RunDenizen { .. } => Ring::HostOnly,
    }
}

/// May `subject` emit `action` under `authority`? The single deny point for
/// the envelope lane: host-only refuses structurally, everything else asks
/// the authority for the ring's path (write mode — an emission acts).
/// The `Err` names the ring, so a denial is attributable by capability.
pub fn emit_allowed(
    authority: &PrefixAuthority,
    subject: Subject,
    action: &Action,
) -> Result<(), String> {
    let ring = ring_of(action);
    let Some(path) = ring.path() else {
        return Err(format!(
            "{}: gate management is host-only; no grantable path exists",
            ring.name()
        ));
    };
    if servitor::AuthorityProvider::covers(authority, subject, path, Mode::Write) {
        Ok(())
    } else {
        Err(format!("{}: not covered by this denizen's grant", ring.name()))
    }
}

/// Every ring, in privilege order (least first). Host-only is deliberately
/// absent: it is not a choice a review can offer.
pub const GRANTABLE_RINGS: [Ring; 4] = [Ring::Navigate, Ring::Panes, Ring::Dispatch, Ring::Session];

/// The interface-shaped names of the rings this subject's authority actually
/// covers — the `caps.granted()` answer a component reads to skip a feature
/// instead of emitting into a denial. The grant stays authoritative; this is
/// the guest's read-only window onto it.
pub fn granted_ring_names(authority: &PrefixAuthority, subject: Subject) -> Vec<String> {
    GRANTABLE_RINGS
        .iter()
        .filter(|ring| {
            ring.path().is_some_and(|path| {
                servitor::AuthorityProvider::covers(authority, subject, path, Mode::Write)
            })
        })
        .map(|ring| format!("mere:script/actions#{}", ring.name()))
        .collect()
}

/// An envelope that failed to become an action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvelopeError {
    /// No action by this name (or its payload shape is not yet decodable in
    /// this build) — loud, never a silent drop.
    Unknown(String),
    /// The name is known but the payload did not parse for it.
    Malformed(String),
}

/// Decode an emission envelope (`name` kebab-case, `payload` JSON, `""` for
/// unit actions) into an [`Action`]. Decoding is NOT authority — a decoded
/// host-only action still dies at [`emit_allowed`]; an undecodable name is a
/// loud [`EnvelopeError::Unknown`]. The decodable set grows with need; the
/// CLASSIFIER is what must stay total.
pub fn decode_envelope(name: &str, payload: &str) -> Result<Action, EnvelopeError> {
    fn field(payload: &str, key: &str) -> Result<serde_json::Value, EnvelopeError> {
        let value: serde_json::Value = serde_json::from_str(payload)
            .map_err(|e| EnvelopeError::Malformed(format!("payload: {e}")))?;
        value
            .get(key)
            .cloned()
            .ok_or_else(|| EnvelopeError::Malformed(format!("missing field `{key}`")))
    }
    fn string(payload: &str, key: &str) -> Result<String, EnvelopeError> {
        field(payload, key)?
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| EnvelopeError::Malformed(format!("field `{key}`: expected a string")))
    }
    fn float(payload: &str, key: &str) -> Result<f32, EnvelopeError> {
        field(payload, key)?
            .as_f64()
            .map(|f| f as f32)
            .ok_or_else(|| EnvelopeError::Malformed(format!("field `{key}`: expected a number")))
    }
    fn id(payload: &str, key: &str) -> Result<uuid::Uuid, EnvelopeError> {
        string(payload, key)?
            .parse()
            .map_err(|e| EnvelopeError::Malformed(format!("field `{key}`: {e}")))
    }
    fn member(payload: &str) -> Result<uuid::Uuid, EnvelopeError> {
        id(payload, "member")
    }

    Ok(match name {
        // navigate
        "open-address" => Action::OpenAddress(string(payload, "url")?),
        "nav-back" => Action::NavBack,
        "nav-forward" => Action::NavForward,
        "reload" => Action::Reload,
        // panes
        "new-window" => Action::NewWindow,
        "tear-out-active-pane" => Action::TearOutActivePane,
        "close-active-pane" => Action::CloseActivePane,
        "toggle-maximize-pane" => Action::ToggleMaximizePane,
        "open-in-workbench" => Action::OpenInWorkbench,
        "tear-out-tile" => Action::TearOutTile { member: member(payload)? },
        "workbench-activate" => Action::WorkbenchActivate(member(payload)?),
        "close-workbench-tile" => Action::CloseWorkbenchTile,
        // dispatch
        "reseed-layout" => Action::ReseedLayout,
        "fit-view" => Action::FitView,
        "toggle-isometric" => Action::ToggleIsometric,
        "orbit-by" => Action::OrbitBy(float(payload, "radians")?),
        "tilt-by" => Action::TiltBy(float(payload, "delta")?),
        "toggle-height-by-degree" => Action::ToggleHeightByDegree,
        "toggle-physics" => Action::TogglePhysics,
        "toggle-size-by-recency" => Action::ToggleSizeByRecency,
        "toggle-node-content" => Action::ToggleNodeContent,
        "omnibar-insert" => Action::OmnibarInsert(string(payload, "text")?),
        "omnibar-commit" => Action::OmnibarCommit,
        "omnibar-close" => Action::OmnibarClose,
        // session
        "save-session" => Action::SaveSession,
        "fork-node" => Action::ForkNode { member: member(payload)? },
        "fork-focused-node" => Action::ForkFocusedNode,
        "new-session" => Action::NewSession,
        "switch-session" => {
            Action::SwitchSession(frisket::SessionId::from_uuid(id(payload, "id")?))
        }
        "close-session" => Action::CloseSession,
        "delete-focused-node" => Action::DeleteFocusedNode,
        "recover-deleted-node" => Action::RecoverDeletedNode(member(payload)?),
        "empty-recycle-bin" => Action::EmptyRecycleBin,
        "recover-session" => {
            Action::RecoverSession(frisket::SessionId::from_uuid(id(payload, "id")?))
        }
        // host-only (decodable so the DENIAL is exact and attributable)
        "install-denizen" => Action::InstallDenizen { path: string(payload, "path")? },
        "confirm-install-denizen" => Action::ConfirmInstallDenizen,
        "cancel-install-denizen" => Action::CancelInstallDenizen,
        "run-denizen" => Action::RunDenizen { member: member(payload)? },
        other => return Err(EnvelopeError::Unknown(other.to_string())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use servitor::Grant;

    fn subject() -> Subject {
        Subject::new([9u8; 32])
    }

    fn full_app_authority() -> PrefixAuthority {
        // Every grantable app ring, deliberately including a grant WIDER than
        // any single ring: host-only must resist even this.
        PrefixAuthority::default().with_grant(Grant::new(subject(), "app/", Mode::Write))
    }

    #[test]
    fn every_ring_but_host_only_names_a_grantable_path() {
        for ring in [Ring::Navigate, Ring::Panes, Ring::Dispatch, Ring::Session] {
            assert!(ring.path().is_some(), "{} must be grantable", ring.name());
        }
        assert_eq!(Ring::HostOnly.path(), None, "the structural floor");
    }

    #[test]
    fn a_covered_emission_passes_and_an_uncovered_one_names_its_ring() {
        let narrow = PrefixAuthority::default().with_grant(Grant::new(
            subject(),
            "app/navigate",
            Mode::Write,
        ));
        assert!(
            emit_allowed(&narrow, subject(), &Action::OpenAddress("https://a".into())).is_ok()
        );
        let denial = emit_allowed(&narrow, subject(), &Action::CloseSession)
            .expect_err("session is not covered");
        assert!(denial.contains("session"), "the denial names the ring: {denial}");
    }

    #[test]
    fn gate_management_resists_even_a_total_app_grant() {
        // The self-escalation guard: a component confirming its own install
        // review must be impossible under ANY authority.
        let authority = full_app_authority();
        for action in [
            Action::ConfirmInstallDenizen,
            Action::CancelInstallDenizen,
            Action::InstallDenizen { path: "x.lua".into() },
            Action::RunDenizen { member: uuid::Uuid::from_u128(1) },
        ] {
            let denial = emit_allowed(&authority, subject(), &action)
                .expect_err("host-only must refuse");
            assert!(denial.contains("host-only"), "{denial}");
        }
    }

    #[test]
    fn envelopes_decode_and_misfires_are_loud() {
        assert_eq!(
            decode_envelope("open-address", r#"{"url": "https://a.test"}"#),
            Ok(Action::OpenAddress("https://a.test".to_string()))
        );
        assert_eq!(decode_envelope("fit-view", ""), Ok(Action::FitView));
        assert!(matches!(
            decode_envelope("open-address", r#"{}"#),
            Err(EnvelopeError::Malformed(_))
        ));
        assert!(matches!(
            decode_envelope("summon-the-kraken", ""),
            Err(EnvelopeError::Unknown(_))
        ));
        // A host-only name DECODES (so the denial downstream is exact);
        // authority is emit_allowed's job, not the decoder's.
        assert_eq!(
            decode_envelope("confirm-install-denizen", ""),
            Ok(Action::ConfirmInstallDenizen)
        );
    }

    #[test]
    fn decoded_envelopes_classify_across_all_grantable_rings() {
        for (name, payload, ring) in [
            ("open-address", r#"{"url": "https://a"}"#, Ring::Navigate),
            ("new-window", "", Ring::Panes),
            ("fit-view", "", Ring::Dispatch),
            ("close-session", "", Ring::Session),
            ("run-denizen", r#"{"member": "00000000-0000-0000-0000-000000000001"}"#, Ring::HostOnly),
        ] {
            let action = decode_envelope(name, payload).expect(name);
            assert_eq!(ring_of(&action), ring, "{name}");
        }
    }
}
