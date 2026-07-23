//! The wasm denizen lane: an `app-core` component acting on merecat through
//! the action ENVELOPE, ring-gated at every emission.
//!
//! This is the second face of the ONE grant (participant gate B2/B3): the
//! piccolo lane derives `ScriptCapabilities` from the denizen's authority,
//! and this lane checks the same authority per emission via [`crate::ring`].
//! Neither lane carries a feature flag that decides what a denizen may do —
//! the grant decides, and the review is where a grant comes from.
//!
//! The sink COLLECTS rather than applying: a host function cannot hold
//! `&mut App` while the app sits inside a wasm call. So an allowed emission
//! is queued, and the caller lowers the queue through the ordinary Action
//! spine after the turn, with the journal scoped to the denizen's subject —
//! exactly the shape `RunDenizen` already uses for piccolo.

use std::path::Path;
use std::time::Duration;

use app_host::{ActionSink, AppScript, Refusal, Watchdog};
use servitor::{PrefixAuthority, Subject};
use wasmtime::StoreLimitsBuilder;

use crate::action::Action;
use crate::ring::{self, EnvelopeError};

/// How long a component turn may run before the epoch deadline trips, in
/// watchdog ticks (5ms each) — generous for a control turn, hard against a
/// runaway loop.
const EPOCH_DEADLINE_TICKS: u64 = 200;

/// The memory ceiling one component instance may allocate.
const MEMORY_CEILING: usize = 64 * 1024 * 1024;

/// The ring gate as an [`ActionSink`]: decode the envelope, classify it, ask
/// the authority, and queue what passes.
pub struct RingSink {
    authority: PrefixAuthority,
    subject: Subject,
    /// Emissions accepted for lowering, in emission order.
    pub accepted: Vec<Action>,
    /// Every refusal, for the run's honest report.
    pub refusals: Vec<String>,
}

impl RingSink {
    pub fn new(authority: PrefixAuthority, subject: Subject) -> Self {
        Self {
            authority,
            subject,
            accepted: Vec::new(),
            refusals: Vec::new(),
        }
    }
}

impl ActionSink for RingSink {
    fn emit(&mut self, name: &str, payload: &str) -> Result<(), Refusal> {
        let action = match ring::decode_envelope(name, payload) {
            Ok(action) => action,
            Err(EnvelopeError::Unknown(name)) => {
                let refusal = Refusal::Unknown(name);
                self.refusals.push(format!("{refusal:?}"));
                return Err(refusal);
            }
            Err(EnvelopeError::Malformed(why)) => {
                let refusal = Refusal::Malformed(why);
                self.refusals.push(format!("{refusal:?}"));
                return Err(refusal);
            }
        };
        // Decoding is not authority: a host-only action decodes fine and dies
        // here, so the denial is exact and names its ring.
        if let Err(why) = ring::emit_allowed(&self.authority, self.subject, &action) {
            self.refusals.push(why.clone());
            return Err(Refusal::Denied(why));
        }
        self.accepted.push(action);
        Ok(())
    }
}

/// One component run's outcome: the actions to lower, the refusals to report,
/// and the guest's log lines.
pub struct ComponentRun {
    pub actions: Vec<Action>,
    pub refusals: Vec<String>,
    pub logs: Vec<String>,
}

/// Run a resident component: instantiate `app-core` under an epoch-guarded
/// engine, `activate`, deliver one `(kind, payload)` event, `deactivate`, and
/// return what the ring gate accepted. A trap (runaway loop, allocation past
/// the ceiling) is contained and reported as an `Err`; the host survives.
pub fn run(
    path: &Path,
    authority: &PrefixAuthority,
    subject: Subject,
    kind: &str,
    payload: &str,
) -> Result<ComponentRun, String> {
    let engine = app_host::guarded_engine().map_err(|err| format!("engine: {err}"))?;
    let _watchdog = Watchdog::start(engine.clone(), Duration::from_millis(5));
    // What the guest is TOLD it holds (`caps.granted()`): the rings its
    // authority actually covers, so a well-written component skips a feature
    // instead of emitting into a denial.
    let granted = ring::granted_ring_names(authority, subject);
    let mut script = AppScript::attach_blocking(
        &engine,
        path,
        RingSink::new(authority.clone(), subject),
        granted,
        StoreLimitsBuilder::new().memory_size(MEMORY_CEILING).build(),
        Some(EPOCH_DEADLINE_TICKS),
    )
    .map_err(|err| format!("instantiate: {err}"))?;
    script
        .activate_blocking()
        .map_err(|err| format!("activate: {err}"))?;
    script
        .on_event_blocking(kind, payload)
        .map_err(|err| format!("turn: {err}"))?;
    // A failed deactivate is reported, never fatal: the turn already happened
    // and its accepted actions are real.
    if let Err(err) = script.deactivate_blocking() {
        tracing::warn!(%err, "component deactivate failed");
    }
    let logs = script.logs().to_vec();
    let sink = script.sink();
    Ok(ComponentRun {
        actions: sink.accepted.clone(),
        refusals: sink.refusals.clone(),
        logs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use servitor::{Grant, Mode};

    fn subject() -> Subject {
        Subject::new([3u8; 32])
    }

    #[test]
    fn the_sink_queues_granted_emissions_and_refuses_the_rest() {
        let authority = PrefixAuthority::default()
            .with_grant(Grant::new(subject(), "app/navigate", Mode::Write));
        let mut sink = RingSink::new(authority, subject());

        assert!(sink.emit("open-address", r#"{"url": "https://a.test"}"#).is_ok());
        assert!(matches!(
            sink.emit("close-session", ""),
            Err(Refusal::Denied(why)) if why.contains("session")
        ));
        assert!(matches!(
            sink.emit("confirm-install-denizen", ""),
            Err(Refusal::Denied(why)) if why.contains("host-only")
        ));
        assert!(matches!(
            sink.emit("summon-the-kraken", ""),
            Err(Refusal::Unknown(_))
        ));

        assert_eq!(sink.accepted.len(), 1, "only the granted ring queued");
        assert!(matches!(sink.accepted[0], Action::OpenAddress(_)));
        assert_eq!(sink.refusals.len(), 3);
    }
}
