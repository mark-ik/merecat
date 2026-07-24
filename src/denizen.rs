//! Denizen residency (participant gate B1, the merecat half): install a local
//! scenario pack as a resident helper, review its grant visibly, run it from
//! the palette, and read its edits back attributed.
//!
//! The substrate is already built and this module only wires it: the node IS
//! the denizen (the `denizen.binding` facet carries subject + kind — agency;
//! the world it bears hangs on `Node.nested` — structure, the kernel's
//! `GraphBearing` impl), its inner world is a chartulary `GraphLog` the `servitor::Gate`
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
use servitor::{Cap, Gate, Grant, GrantTable, Mode, Subject};
use uuid::Uuid;

use crate::app::App;

/// The facet carrying a scenario denizen's runnable source (merecat's own
/// namespace beside `denizen.binding`; the binding stays app-agnostic).
pub const SCENARIO_SOURCE_FACET: &str = "scenario.source";

/// The facet naming a component denizen's `.wasm` file, relative to the
/// session's `denizens/` dir. The bytes live on disk (never in a facet); the
/// facet is the pointer, exactly like the world's log id is a pointer.
pub const COMPONENT_FACET: &str = "component.file";

/// The capability covering the app's READ face (the observe tier). Not an
/// emission ring, since nothing is dispatched by reading, so every resident
/// gets it and the rings are what the review actually asks for.
pub fn read_cap() -> Cap {
    Cap::Power("read".to_string())
}

/// The capability a resident holds over its OWN nested world: a scope, not a
/// power, because a world is a place with an unbounded interior and prefix
/// containment is exactly what is wanted there.
pub fn world_cap() -> Cap {
    Cap::Scope(servitor::ScopePath::parse(SCENARIO_SCOPE).expect("a valid scope"))
}

/// The capability path a rung-1 scenario denizen is granted over its own
/// nested world (`Mode::Write`). The visible review names it.
pub const SCENARIO_SCOPE: &str = "scenario/";

/// The piccolo step budget a denizen run gets — generous for a control
/// script, hard against a runaway loop.
pub const RUN_BUDGET: u64 = 20_000;

/// What a pack actually IS: a control script's source, or a wasm component's
/// bytes. Both are content-addressed the same way (blake3 over the bytes), so
/// identity does not care which lane runs it.
#[derive(Clone, Debug, PartialEq)]
pub enum PackBody {
    /// A piccolo control script (`.lua`).
    Scenario(String),
    /// An `app-core` wasm component (`.wasm`).
    Component(Vec<u8>),
}

impl PackBody {
    /// The denizen kind this body resides as.
    pub fn kind(&self) -> session_runtime::DenizenKind {
        match self {
            PackBody::Scenario(_) => session_runtime::DenizenKind::Scenario,
            PackBody::Component(_) => session_runtime::DenizenKind::Pack,
        }
    }

    /// How the review names the runnable. Short on purpose: the whole ask
    /// has to fit one palette row without clipping, and what matters in it
    /// is the RINGS.
    pub fn noun(&self) -> &'static str {
        match self {
            PackBody::Scenario(_) => "lua",
            PackBody::Component(_) => "wasm",
        }
    }
}

/// A staged install awaiting the VISIBLE grant review: nothing is minted, no
/// grant exists, until the user confirms from the palette.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingInstall {
    /// Where the pack came from (display + provenance).
    pub path: PathBuf,
    /// The denizen's display label (the file stem).
    pub label: String,
    /// The runnable body.
    pub body: PackBody,
    /// The content-derived subject.
    pub subject: Subject,
    /// The action RINGS this install would grant — the default profile,
    /// PRESELECTED for the review, never silently granted: the confirm row
    /// names them, and only confirming turns the ask into a grant.
    pub rings: Vec<crate::ring::Ring>,
}

/// The default ring profile a staged pack arrives with. Control rings
/// (navigate / panes / dispatch) are what a helper needs to be useful; the
/// session ring (fork / close / delete / recover) is destructive, so it is
/// never preselected — a pack that wants it must be granted it deliberately.
/// Host-only is not a profile choice at all: no grant can cover it.
pub fn default_rings() -> Vec<crate::ring::Ring> {
    use crate::ring::Ring;
    vec![Ring::Navigate, Ring::Panes, Ring::Dispatch]
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
    pub authority: GrantTable,
    pub gate: Gate,
    /// Residents whose world id came from a LEGACY binding facet
    /// (`nested_log` written before the containment ruling) rather than
    /// `Node.nested`. The adopt path heals each: set the node's `nested`,
    /// rewrite the binding without the field.
    pub legacy_heals: Vec<(Uuid, String)>,
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
    let bytes = std::fs::read(path).map_err(|err| format!("unreadable pack: {err}"))?;
    if bytes.is_empty() {
        return Err("the pack is empty".to_string());
    }
    // The subject is the bytes' blake3 either way: the same pack is the same
    // denizen whichever lane runs it, and an edited pack faces a fresh review.
    let subject = Subject::new(*blake3::hash(&bytes).as_bytes());
    let is_component = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wasm"));
    let body = if is_component {
        PackBody::Component(bytes)
    } else {
        let source = String::from_utf8(bytes)
            .map_err(|_| "the pack is not valid UTF-8 (a component must end in .wasm)".to_string())?;
        if source.trim().is_empty() {
            return Err("the pack is empty".to_string());
        }
        PackBody::Scenario(source)
    };
    let label = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("denizen")
        .to_string();
    Ok(PendingInstall {
        path: path.to_path_buf(),
        label,
        body,
        subject,
        rings: default_rings(),
    })
}

/// Where a component denizen's `.wasm` lives: `sessions/<id>/denizens/<subject>.wasm`
/// (beside the worlds — a resident's whole substance in one place).
pub fn component_path(session_dir: &Path, file: &str) -> PathBuf {
    session_dir.join("denizens").join(file)
}

/// The review line the palette shows on the Confirm row — the ASK, visible
/// before any grant exists.
pub fn review_line(pending: &PendingInstall) -> String {
    let rings = pending
        .rings
        .iter()
        .map(|r| r.name())
        .collect::<Vec<_>>()
        .join(", ");
    // One row, no clipping: the label, the lane, and the RINGS this install
    // would grant. `own world` stands for the `scenario/` scope every
    // resident gets over its own nested graph.
    format!(
        "Install {} ({}) — grants: {}, own world — Confirm",
        pending.label,
        pending.body.noun(),
        rings
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

/// Where an ARCHIVED world sits while its bearer is in the recycle bin:
/// `sessions/<id>/denizens/archive/<log-id>.json` (the file-level echo of
/// chartulary's `archive/nested/...` slot convention).
pub fn archived_world_path(session_dir: &Path, log_id: &str) -> PathBuf {
    session_dir
        .join("denizens")
        .join("archive")
        .join(format!("{log_id}.json"))
}

/// Archive a world: move its live file to the archive slot (archive-never-
/// orphan — the move happens BEFORE the bearing node leaves the graph, and a
/// failure aborts the delete). A world with no live file is fine: there is
/// nothing to move, recovery starts it empty as always.
pub fn archive_world(session_dir: &Path, log_id: &str) -> std::io::Result<()> {
    let live = nested_log_path(session_dir, log_id);
    if !live.is_file() {
        return Ok(());
    }
    let archived = archived_world_path(session_dir, log_id);
    if let Some(parent) = archived.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&live, &archived)
}

/// Recover a world: move its archived file back to the live slot. A missing
/// archive is fine (the world had no file, or an older build deleted without
/// archiving) — the resident rebuilds on an empty world.
pub fn unarchive_world(session_dir: &Path, log_id: &str) -> std::io::Result<()> {
    let archived = archived_world_path(session_dir, log_id);
    if !archived.is_file() {
        return Ok(());
    }
    let live = nested_log_path(session_dir, log_id);
    if let Some(parent) = live.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&archived, &live)
}

/// Complete a forget: remove the archived world of a purged tombstone
/// (emptying the bin, or athanor's retirement pass). Best-effort — the
/// tombstone is already gone; a leftover file is litter, not data loss.
pub fn purge_archived_world(session_dir: &Path, log_id: &str) {
    let archived = archived_world_path(session_dir, log_id);
    if archived.is_file()
        && let Err(err) = std::fs::remove_file(&archived)
    {
        tracing::warn!(%err, log_id, "failed to purge an archived denizen world");
    }
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
/// `denizen.binding` facet names a resident; the graph node's `nested` field
/// names its borne world (structure), whose log loads from disk (or starts
/// empty), and its authority derives from the **grant projections** in that
/// log — the projection is the readable record, the provider the derived
/// index, so authority is never stored twice.
///
/// A binding written before the containment ruling named the world itself
/// (`legacy_nested_log`); such a resident still rebuilds, and the member goes
/// on [`Denizens::legacy_heals`] so the adopt path can move the pointer onto
/// the node and rewrite the facet without it.
pub fn rebuild(
    app_facets: &session_runtime::NodeFacetStore,
    graph: &mere::kernel::graph::Graph,
    session_dir: &Path,
) -> Denizens {
    let mut denizens = Denizens::default();
    for (member, binding) in session_runtime::read_denizen_bindings(app_facets) {
        let Ok(raw) = hex_to_bytes(&binding.subject) else {
            tracing::warn!(member = %member, "denizen binding with unparseable subject; skipped");
            continue;
        };
        let subject = Subject::new(raw);
        let borne = graph
            .get_node_key_by_id(member)
            .and_then(|key| graph.get_node(key))
            .and_then(|node| node.nested.as_ref())
            .map(|log| log.as_str().to_string());
        let log_id = match borne {
            Some(id) => id,
            None if !binding.legacy_nested_log.is_empty() => {
                denizens
                    .legacy_heals
                    .push((member, binding.legacy_nested_log.clone()));
                binding.legacy_nested_log.clone()
            }
            None => {
                tracing::warn!(member = %member, "denizen binding on a node bearing no world; skipped");
                continue;
            }
        };
        let nested = load_nested(session_dir, &log_id)
            .unwrap_or_else(|| GraphLog::with_id(LogId::new(log_id.clone())));
        // Derive the authority from the projections the gate wrote. The
        // projection is a LOSSLESS record now (capability + mode + subject in
        // explicit tags), so the grant is read back exactly rather than
        // reconstructed with a hardcoded mode.
        for (_, node) in nested.graph().nodes() {
            if let Some(grant) = servitor::read_projection(node) {
                denizens.authority = std::mem::take(&mut denizens.authority).with_grant(grant);
                continue;
            }
            // Pre-capability-model projections carried only the path in the
            // node id. Heal them: an `app/<ring>` path is that ring's power,
            // anything else is the scope it always was.
            if let Some(path) = node.id.strip_prefix(servitor::GRANT_PREFIX) {
                let cap = crate::ring::Ring::from_legacy_path(path)
                    .and_then(|ring| ring.cap())
                    .or_else(|| Cap::parse(path).ok());
                if let Some(cap) = cap {
                    denizens.authority = std::mem::take(&mut denizens.authority)
                        .with_grant(Grant::new(subject, cap, Mode::Write));
                }
            }
        }
        let label = app_facets
            .get(&member, &chartulary::FacetId::new("scenario.label"))
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| log_id[..8.min(log_id.len())].to_string());
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
    // The borne world is STRUCTURE: it hangs on the node itself
    // (`Node.nested`, journaled through the delta spine), not on the facet.
    let _ = app
        .canvas
        .set_node_nested_for(member, Some(LogId::new(hex.clone())));

    // The binding + source + label facets: durable agency truth.
    session_runtime::write_denizen_binding(
        &mut app.facets,
        member,
        &session_runtime::DenizenBinding::new(hex.clone(), pending.body.kind()),
    );
    // The runnable: a script's source rides a facet; a component's bytes ride
    // the disk beside the worlds, with the facet as the pointer.
    match &pending.body {
        PackBody::Scenario(source) => {
            let _ = app.facets.set(
                member,
                chartulary::FacetId::new(SCENARIO_SOURCE_FACET),
                serde_json::json!(source),
                &chartulary::AcceptAll,
            );
        }
        PackBody::Component(bytes) => {
            let file = format!("{hex}.wasm");
            let target = component_path(&app.session_dir(), &file);
            let written = (|| -> std::io::Result<()> {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, bytes)
            })();
            match written {
                Ok(()) => {
                    let _ = app.facets.set(
                        member,
                        chartulary::FacetId::new(COMPONENT_FACET),
                        serde_json::json!(file),
                        &chartulary::AcceptAll,
                    );
                }
                Err(err) => {
                    tracing::warn!(%err, path = ?target, "failed to store the component");
                }
            }
        }
    }
    let _ = app.facets.set(
        member,
        chartulary::FacetId::new("scenario.label"),
        serde_json::json!(pending.label),
        &chartulary::AcceptAll,
    );

    // The nested world: fresh log, every granted path projected by the gate
    // (read-only, gate-authored — the browsable record authority derives
    // from). What is granted is exactly what the review named: the denizen's
    // own world, the read face, and ONE PATH PER PRESELECTED RING. No blanket
    // `app/` grant — an unnamed ring is an ungranted ring, and the session
    // ring only appears here if the review asked for it.
    let mut nested = GraphLog::with_id(LogId::new(hex.clone()));
    let mut grants = vec![
        Grant::new(subject, world_cap(), Mode::Write),
        Grant::new(subject, read_cap(), Mode::Write),
    ];
    grants.extend(
        pending
            .rings
            .iter()
            .filter_map(|ring| ring.cap())
            .map(|cap| Grant::new(subject, cap, Mode::Write)),
    );
    for grant in &grants {
        if let Err(err) = app.denizens.gate.project_grant(&mut nested, grant) {
            tracing::warn!(?err, "failed to project an install grant");
        }
    }
    save_nested(&app.session_dir(), &hex, &nested);

    let mut authority = std::mem::take(&mut app.denizens.authority);
    for grant in grants {
        authority = authority.with_grant(grant);
    }
    app.denizens.authority = authority;
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
        let review = review_line(&a);
        assert!(review.contains("grants:"), "the ask is visible: {review}");
        for ring in default_rings() {
            assert!(review.contains(ring.name()), "the ask names {}: {review}", ring.name());
        }
        assert!(
            review.chars().count() < 96,
            "the ask must fit one palette row without clipping: {review}"
        );

        std::fs::write(&path, "mere.open('mere://other')").unwrap();
        let c = stage_install(&path).unwrap();
        assert_ne!(a.subject, c.subject, "a modified script is a different subject");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_binding_rebuilds_and_asks_for_a_heal() {
        // A binding written before the containment ruling names the world in
        // the facet. The resident still rebuilds (no one is orphaned by an
        // upgrade), and the member is listed for the adopt-path heal that
        // moves the pointer onto `Node.nested`.
        let dir = std::env::temp_dir().join(format!("merecat-legacy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let member = Uuid::from_u128(0xa);
        let mut store = session_runtime::NodeFacetStore::new();
        store
            .set(
                member,
                chartulary::FacetId::new(session_runtime::DENIZEN_BINDING),
                serde_json::json!({
                    "subject": "aa".repeat(32),
                    "nested_log": "aa".repeat(32),
                    "kind": "scenario",
                }),
                &chartulary::AcceptAll,
            )
            .unwrap();

        let graph = mere::kernel::graph::Graph::new();
        let denizens = rebuild(&store, &graph, &dir);
        assert_eq!(denizens.residents.len(), 1, "the legacy resident survives");
        assert_eq!(
            denizens.legacy_heals,
            vec![(member, "aa".repeat(32))],
            "and is queued for the one-time heal"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nested_logs_round_trip_through_disk() {
        let dir = std::env::temp_dir().join(format!("merecat-nested-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gate = Gate::new();
        let subject = Subject::new([7u8; 32]);
        let mut nested = GraphLog::with_id(LogId::new("aa".repeat(32)));
        gate.project_grant(&mut nested, &Grant::new(subject, world_cap(), Mode::Write))
            .unwrap();

        save_nested(&dir, &"aa".repeat(32), &nested);
        let restored = load_nested(&dir, &"aa".repeat(32)).expect("log restored");
        assert_eq!(restored.revision(), nested.revision());
        assert!(
            restored
                .graph()
                .key_of(&Gate::projection_id(&world_cap()))
                .is_some(),
            "the grant projection survived the round trip"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
