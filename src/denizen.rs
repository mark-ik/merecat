//! Denizen residency (participant gate B1, the merecat half): install a local
//! scenario pack as a resident helper, review its grant visibly, run it from
//! the palette, and read its edits back attributed.
//!
//! The substrate is already built and this module only wires it: the node IS
//! the denizen (the `denizen.binding` facet carries subject + nested-log id +
//! kind), its inner world is a chartulary `GraphLog` the `servitor::Gate`
//! commits into (grant projections read-only, petitions attributed and
//! revision-checked), and its runnable body is a piccolo control script whose
//! emitted Actions lower through the ordinary spine — under the denizen's
//! author in mere's attributed `GraphJournal`.
//!
//! Identity: the subject is **content-derived** — `blake3(source)` is the
//! 32-byte keyholder — so the same script is the same denizen everywhere, and
//! a modified script is a different subject facing a fresh grant review.
//! (Signed personae subjects arrive with packs at B4; the gate does not care
//! which mints the bytes.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chartulary::{Container, GraphLog, Relation};
use codicil::{Codicil, LogId};
use servitor::{Gate, Grant, Mode, PrefixAuthority, Subject};
use uuid::Uuid;

use crate::app::App;

/// The facet carrying a scenario denizen's runnable source (merecat's own
/// namespace beside `denizen.binding`; the binding stays app-agnostic).
pub const SCENARIO_SOURCE_FACET: &str = "scenario.source";

/// The capability path a rung-1 scenario denizen is granted over its own
/// nested world (`Mode::Write`). The visible review names it.
pub const SCENARIO_SCOPE: &str = "scenario/";

/// The piccolo step budget a denizen run gets — generous for a control
/// script, hard against a runaway loop.
pub const RUN_BUDGET: u64 = 20_000;

/// A staged install awaiting the VISIBLE grant review: nothing is minted, no
/// grant exists, until the user confirms from the palette.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingInstall {
    /// Where the pack came from (display + provenance).
    pub path: PathBuf,
    /// The denizen's display label (the file stem).
    pub label: String,
    /// The runnable source.
    pub source: String,
    /// The content-derived subject.
    pub subject: Subject,
}

/// One resident denizen's live half: its subject and its nested world,
/// rebuilt from the binding facet + the persisted log on adopt.
pub struct Resident {
    pub subject: Subject,
    pub label: String,
    pub nested: GraphLog<Container, Relation>,
}

/// The session's denizen runtime: residents by member node, the authority
/// provider the gate consults, and the gate itself. Rebuilt on adopt; the
/// facts it derives from (binding facets + nested logs) are the durable truth.
#[derive(Default)]
pub struct Denizens {
    pub residents: HashMap<Uuid, Resident>,
    pub authority: PrefixAuthority,
    pub gate: Gate,
}

impl Denizens {
    /// Whether any denizen resides in the session.
    pub fn is_empty(&self) -> bool {
        self.residents.is_empty()
    }
}

/// Stage a `.lua` file as a pending install: read it, derive the subject,
/// and surface the review. `Err` is a human-readable refusal (unreadable
/// file, empty source).
pub fn stage_install(path: &Path) -> Result<PendingInstall, String> {
    let source =
        std::fs::read_to_string(path).map_err(|err| format!("unreadable pack: {err}"))?;
    if source.trim().is_empty() {
        return Err("the pack is empty".to_string());
    }
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("denizen")
        .to_string();
    let subject = Subject::new(*blake3::hash(source.as_bytes()).as_bytes());
    Ok(PendingInstall {
        path: path.to_path_buf(),
        label,
        source,
        subject,
    })
}

/// The review line the palette shows on the Confirm row — the ASK, visible
/// before any grant exists.
pub fn review_line(pending: &PendingInstall) -> String {
    format!(
        "Install {}: may run control scripts and write {} in its own world — Confirm",
        pending.label, SCENARIO_SCOPE
    )
}

/// The denizen node's address: subject-derived, so the same pack is the same
/// node identity-wise across installs.
pub fn denizen_url(subject: Subject) -> String {
    format!("mere://denizen/{}", &subject.to_hex()[..16])
}

/// Where a denizen's nested log persists, beside the session's other state:
/// `sessions/<id>/denizens/<log-id>.json`.
pub fn nested_log_path(session_dir: &Path, log_id: &str) -> PathBuf {
    session_dir.join("denizens").join(format!("{log_id}.json"))
}

/// Persist a resident's nested log (whole-log JSON; the log IS the graph).
/// Best-effort like every sidecar: a failed save warns, never panics.
pub fn save_nested(session_dir: &Path, log_id: &str, nested: &GraphLog<Container, Relation>) {
    let target = nested_log_path(session_dir, log_id);
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(nested.log()).map_err(std::io::Error::other)?;
        std::fs::write(&target, json)
    })();
    if let Err(err) = result {
        tracing::warn!(%err, path = ?target, "failed to persist a denizen's nested log");
    }
}

/// Load a resident's nested log; `None` when absent or unreadable (the
/// denizen then starts on an empty world — its binding still stands).
pub fn load_nested(session_dir: &Path, log_id: &str) -> Option<GraphLog<Container, Relation>> {
    let path = nested_log_path(session_dir, log_id);
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<Codicil<chartulary::Batch<Container, Relation>>>(&text) {
        Ok(log) => Some(GraphLog::replay(log)),
        Err(err) => {
            tracing::warn!(%err, path = ?path, "failed to parse a denizen's nested log");
            None
        }
    }
}

/// Rebuild the denizen runtime from durable truth on adopt: every
/// `denizen.binding` facet names a resident; its nested log loads from disk
/// (or starts empty), and its authority derives from the **grant projections**
/// in that log — the projection is the readable record, the provider the
/// derived index, so authority is never stored twice.
pub fn rebuild(app_facets: &session_runtime::NodeFacetStore, session_dir: &Path) -> Denizens {
    let mut denizens = Denizens::default();
    for (member, binding) in session_runtime::read_denizen_bindings(app_facets) {
        let Ok(raw) = hex_to_bytes(&binding.subject) else {
            tracing::warn!(member = %member, "denizen binding with unparseable subject; skipped");
            continue;
        };
        let subject = Subject::new(raw);
        let nested = load_nested(session_dir, &binding.nested_log).unwrap_or_else(|| {
            GraphLog::with_id(LogId::new(binding.nested_log.clone()))
        });
        // Derive the authority from the projections the gate wrote: node ids
        // `grant:<path>` under this denizen's subject.
        for (_, node) in nested.graph().nodes() {
            if let Some(path) = node.id.strip_prefix(servitor::GRANT_PREFIX) {
                denizens.authority = std::mem::take(&mut denizens.authority)
                    .with_grant(Grant::new(subject, path, Mode::Write));
            }
        }
        let label = app_facets
            .get(&member, &chartulary::FacetId::new("scenario.label"))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| binding.nested_log[..8.min(binding.nested_log.len())].to_string());
        denizens.residents.insert(
            member,
            Resident {
                subject,
                label,
                nested,
            },
        );
    }
    denizens
}

fn hex_to_bytes(hex: &str) -> Result<[u8; 32], ()> {
    if hex.len() != 64 {
        return Err(());
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| ())?;
        out[i] = u8::from_str_radix(s, 16).map_err(|_| ())?;
    }
    Ok(out)
}

/// Mint the confirmed denizen into the session: the graph node, the binding +
/// source facets, the nested world with its gate-projected grant, and the
/// runtime entry. Returns the member id. (The caller persists: facets ride
/// the ordinary save; the nested log saves here, once, at its birth.)
pub fn install(app: &mut App, pending: PendingInstall) -> Uuid {
    let subject = pending.subject;
    let hex = subject.to_hex();

    // The graph node — minted through the ordinary spine (visit selects it).
    let key = app.canvas.visit(&denizen_url(subject));
    let member = app
        .canvas
        .graph()
        .get_node(key)
        .map(|n| n.id)
        .expect("the just-visited node exists");
    let _ = app.canvas.set_node_title_for(member, pending.label.clone());

    // The binding + source + label facets: durable residency truth.
    session_runtime::write_denizen_binding(
        &mut app.facets,
        member,
        &session_runtime::DenizenBinding::new(
            hex.clone(),
            hex.clone(),
            session_runtime::DenizenKind::Scenario,
        ),
    );
    let _ = app.facets.set(
        member,
        chartulary::FacetId::new(SCENARIO_SOURCE_FACET),
        serde_json::json!(pending.source),
        &chartulary::AcceptAll,
    );
    let _ = app.facets.set(
        member,
        chartulary::FacetId::new("scenario.label"),
        serde_json::json!(pending.label),
        &chartulary::AcceptAll,
    );

    // The nested world: fresh log, grant projected by the gate (read-only,
    // gate-authored — the browsable record authority derives from).
    let mut nested = GraphLog::with_id(LogId::new(hex.clone()));
    let grant = Grant::new(subject, SCENARIO_SCOPE, Mode::Write);
    if let Err(err) = app.denizens.gate.project_grant(&mut nested, &grant) {
        tracing::warn!(?err, "failed to project the install grant");
    }
    save_nested(&app.session_dir(), &hex, &nested);

    app.denizens.authority =
        std::mem::take(&mut app.denizens.authority).with_grant(grant);
    app.denizens.residents.insert(
        member,
        Resident {
            subject,
            label: pending.label,
            nested,
        },
    );
    member
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staged_installs_are_content_derived_and_reviewable() {
        let dir = std::env::temp_dir().join(format!("merecat-denizen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("trail-keeper.lua");
        std::fs::write(&path, "mere.open('mere://kept')").unwrap();

        let a = stage_install(&path).unwrap();
        let b = stage_install(&path).unwrap();
        assert_eq!(a.subject, b.subject, "same source, same subject");
        assert_eq!(a.label, "trail-keeper");
        assert!(review_line(&a).contains("may run control scripts"), "the ask is visible");

        std::fs::write(&path, "mere.open('mere://other')").unwrap();
        let c = stage_install(&path).unwrap();
        assert_ne!(a.subject, c.subject, "a modified script is a different subject");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_logs_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("merecat-nested-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gate = Gate::new();
        let subject = Subject::new([7u8; 32]);
        let mut nested = GraphLog::with_id(LogId::new("aa".repeat(32)));
        gate.project_grant(&mut nested, &Grant::new(subject, SCENARIO_SCOPE, Mode::Write))
            .unwrap();

        save_nested(&dir, &"aa".repeat(32), &nested);
        let restored = load_nested(&dir, &"aa".repeat(32)).expect("log restored");
        assert_eq!(restored.revision(), nested.revision());
        assert!(
            restored
                .graph()
                .key_of(&Gate::projection_id(SCENARIO_SCOPE))
                .is_some(),
            "the grant projection survived the round trip"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
