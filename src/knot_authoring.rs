//! A retained Knot authoring document over Graphshell.
//!
//! The background hub owns the one endpoint process and all carrier traffic.
//! Each visible document owns a local Cambium editor. Keystrokes, selection,
//! undo, IME, highlighting, outline, folds, and preview therefore stay on the
//! UI thread; only Open, Save, and revision refresh cross the carrier.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cambium::{
    AnyView, DomHandle, GenetAppRunner, GenetCtx, GenetElement, Key, KeyEvent, PointerClick,
    StyleRange, button, el, lens, styled_textarea,
};
use genet_layout::{IncrementalLayout, ScrollOffsets};
use genet_scripted_dom::{NodeId, ScriptedDom};
use graphshell::client::{ResolvedContent, ResolvedPresentation};
use graphshell::protocol::{
    AdvertisedAction, CapabilityProfile, EDITABLE_TEXT_SAVE_INTENT, EditableTextV1,
    InsertKnotClipV1, IntentResult, KNOT_CLIP_INSERT_INTENT, PresentationCapability,
    ProjectionSession, SaveTextV1,
};
use graphshell::sessions::RetainedEndpointSession;
use inker::{
    ContentReport, DocumentSession, OutlineEntry, SessionClick, SessionEngine, SessionError,
    SessionLink, SessionScrollKey, SessionSpawnRequest,
};
use knot_editor_host::KnotEditor;
use netrender::Scene;
use sceno::InstanceId;

/// The app-side route id. Knot remains the authority; this names only the
/// retained Turnstone presentation.
pub const ENGINE_ID: &str = "knot.authoring";

const DEFAULT_MAX_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

const KNOT_SHEET: &str = "\
    .knot-root { background-color: rgb(22, 27, 40); color: rgb(205, 212, 226); } \
    .knot-toolbar { display: flex; background-color: rgb(18, 22, 33); \
                    padding: 6px 10px; } \
    .knot-title { color: rgb(205, 212, 226); font-size: 12px; width: 55%; \
                  white-space: nowrap; overflow: hidden; } \
    .knot-status { color: rgb(140, 153, 176); font-size: 12px; width: 25%; \
                   white-space: nowrap; } \
    .knot-save { color: rgb(232, 150, 40); background-color: rgb(28, 34, 50); \
                 border: 1px solid rgb(52, 62, 86); padding: 3px 10px; } \
    .knot-body { display: flex; } \
    .knot-editor-wrap { width: 70%; padding: 10px; } \
    .knot-editor-wrap textarea { color: rgb(218, 224, 236); \
                                 background-color: rgb(25, 30, 44); \
                                 font-size: 13px; white-space: pre-wrap; \
                                 padding: 10px; border: 1px solid rgb(52, 62, 86); } \
    .knot-readout { width: 30%; padding: 10px; color: rgb(172, 183, 204); \
                    font-size: 12px; white-space: pre-wrap; overflow: hidden; } \
    .syntax-heading { color: rgb(240, 179, 94); } \
    .syntax-link { color: rgb(103, 184, 235); } \
    .syntax-strong { color: rgb(235, 239, 247); } \
    .syntax-emphasis { color: rgb(194, 205, 224); } \
    .syntax-codeblock, .syntax-verbatim { color: rgb(141, 207, 160); }";

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct DocumentBinding {
    target: InstanceId,
    save_action: AdvertisedAction,
    clip_action: AdvertisedAction,
    editable: EditableTextV1,
}

struct OpenedDocument {
    registration: u64,
    binding: DocumentBinding,
    events: Receiver<HubEvent>,
}

enum HubCommand {
    Open {
        registration: u64,
        address: String,
        events: Sender<HubEvent>,
        reply: SyncSender<Result<(u64, DocumentBinding), String>>,
    },
    Save {
        registration: u64,
        base_token: Vec<u8>,
        source: String,
    },
    InsertClip {
        address: String,
        clip: PendingClip,
        status: Arc<Mutex<KnotClipStatus>>,
    },
    Reload {
        registration: u64,
    },
    Unregister {
        registration: u64,
    },
}

#[derive(Clone)]
enum HubEvent {
    Remote(DocumentBinding),
    Saved {
        source: String,
        binding: DocumentBinding,
    },
    Reloaded(DocumentBinding),
    Stale,
    Rejected(String),
    Revoked(String),
}

struct Subscriber {
    address: String,
    events: Sender<HubEvent>,
}

struct KnotHub {
    commands: Sender<HubCommand>,
    next_registration: AtomicU64,
}

#[derive(Clone)]
struct PendingClip {
    source_url: String,
    title: Option<String>,
    selector: Option<String>,
    knot_body: String,
}

/// Last known result of the configured Inspector clip destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KnotClipStatus {
    Ready,
    Sending,
    Saved,
    Stale,
    Rejected(String),
}

impl KnotClipStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Ready => "ready".into(),
            Self::Sending => "clipping".into(),
            Self::Saved => "clip saved".into(),
            Self::Stale => "clip target changed; retry".into(),
            Self::Rejected(reason) => reason.clone(),
        }
    }
}

/// UI-thread handle for the endpoint-owned clip action.
#[derive(Clone)]
pub struct KnotClipHandle {
    hub: Arc<KnotHub>,
    target: String,
    status: Arc<Mutex<KnotClipStatus>>,
}

impl KnotClipHandle {
    pub fn insert(&self, clip: inker::DocumentClip) -> Result<(), String> {
        let fragment = mere_import::web_clip::fragment_from_text(
            clip.source_url.clone(),
            clip.title.clone(),
            clip.text,
            clip.selector.clone(),
            clip.links,
        );
        let pending = PendingClip {
            source_url: clip.source_url,
            title: clip.title,
            selector: clip.selector,
            knot_body: mere_import::web_clip::fragment_to_knot_body(&fragment),
        };
        *self.status.lock().map_err(|_| "clip status poisoned")? = KnotClipStatus::Sending;
        self.hub
            .commands
            .send(HubCommand::InsertClip {
                address: self.target.clone(),
                clip: pending,
                status: self.status.clone(),
            })
            .map_err(|_| "Knot authoring worker is unavailable".to_string())
    }

    pub fn status(&self) -> KnotClipStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_else(|_| KnotClipStatus::Rejected("clip status unavailable".into()))
    }

    pub fn target(&self) -> &str {
        &self.target
    }
}

impl KnotHub {
    fn connect(program: PathBuf, args: Vec<OsString>, wake: Wake) -> Result<Arc<Self>, String> {
        let (commands, receiver) = mpsc::channel();
        let (ready_send, ready_receive) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("turnstone-knot-authoring".into())
            .spawn(move || {
                let profile = CapabilityProfile::new([
                    PresentationCapability::EditableText,
                    PresentationCapability::PortableCard,
                ]);
                let retained = RetainedEndpointSession::spawn(program.as_os_str(), &args, profile);
                match retained {
                    Ok(retained) => {
                        let _ = ready_send.send(Ok(()));
                        run_hub(retained, receiver, wake);
                    }
                    Err(error) => {
                        let _ = ready_send.send(Err(error));
                    }
                }
            })
            .map_err(|error| format!("could not start Knot authoring worker: {error}"))?;
        ready_receive
            .recv()
            .map_err(|_| "Knot authoring worker stopped during startup".to_string())??;
        Ok(Arc::new(Self {
            commands,
            next_registration: AtomicU64::new(1),
        }))
    }

    fn open(&self, address: &str) -> Result<OpenedDocument, String> {
        let registration = self.next_registration.fetch_add(1, Ordering::Relaxed);
        let (events_send, events) = mpsc::channel();
        let (reply_send, reply) = mpsc::sync_channel(1);
        self.commands
            .send(HubCommand::Open {
                registration,
                address: address.to_string(),
                events: events_send,
                reply: reply_send,
            })
            .map_err(|_| "Knot authoring worker is unavailable".to_string())?;
        let (confirmed_registration, binding) = reply
            .recv()
            .map_err(|_| "Knot authoring worker stopped while opening".to_string())??;
        debug_assert_eq!(registration, confirmed_registration);
        Ok(OpenedDocument {
            registration,
            binding,
            events,
        })
    }
}

/// One configured endpoint shared by every open Knot document.
pub struct KnotAuthoringEngine {
    hub: Arc<KnotHub>,
    clip_target: Option<String>,
}

impl KnotAuthoringEngine {
    pub fn from_env(wake: Wake) -> Result<Option<Self>, String> {
        let Some(root) = std::env::var_os("TURNSTONE_KNOT_ROOT").map(PathBuf::from) else {
            return Ok(None);
        };
        let program = match std::env::var_os("TURNSTONE_KNOT_ENDPOINT") {
            Some(program) => PathBuf::from(program),
            None => default_endpoint_path()?,
        };
        let max_source_bytes = std::env::var("TURNSTONE_KNOT_MAX_BYTES")
            .ok()
            .map(|value| {
                value.parse::<u64>().map_err(|error| {
                    format!("TURNSTONE_KNOT_MAX_BYTES must be an integer: {error}")
                })
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_SOURCE_BYTES);
        let mode =
            std::env::var("TURNSTONE_KNOT_MODE").unwrap_or_else(|_| "directory-write".into());
        let args = match mode.as_str() {
            "directory-write" => vec![
                "directory-write".into(),
                root.into_os_string(),
                max_source_bytes.to_string().into(),
            ],
            "persona-vault" => {
                let persona = std::env::var_os("TURNSTONE_KNOT_PERSONA").ok_or_else(|| {
                    "TURNSTONE_KNOT_PERSONA is required for persona-vault mode".to_string()
                })?;
                vec![
                    "persona-vault".into(),
                    root.into_os_string(),
                    persona,
                    max_source_bytes.to_string().into(),
                ]
            }
            other => {
                return Err(format!(
                    "unsupported TURNSTONE_KNOT_MODE {other}; expected directory-write or persona-vault"
                ));
            }
        };
        let hub = KnotHub::connect(program, args, wake)?;
        let clip_target = std::env::var("TURNSTONE_KNOT_CLIP_TARGET")
            .ok()
            .filter(|target| !target.trim().is_empty());
        Ok(Some(Self { hub, clip_target }))
    }

    pub fn clip_handle(&self) -> Option<KnotClipHandle> {
        self.clip_target.as_ref().map(|target| KnotClipHandle {
            hub: self.hub.clone(),
            target: target.clone(),
            status: Arc::new(Mutex::new(KnotClipStatus::Ready)),
        })
    }

    #[cfg(test)]
    fn connect_directory(
        program: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        max_source_bytes: u64,
    ) -> Result<Self, String> {
        Ok(Self {
            hub: KnotHub::connect(
                program.into(),
                vec![
                    "directory-write".into(),
                    root.into().into_os_string(),
                    max_source_bytes.to_string().into(),
                ],
                Arc::new(|| {}),
            )?,
            clip_target: None,
        })
    }
}

impl SessionEngine<Scene> for KnotAuthoringEngine {
    fn engine_id(&self) -> &str {
        ENGINE_ID
    }

    fn spawn(
        &self,
        request: &SessionSpawnRequest,
    ) -> Result<Box<dyn DocumentSession<Scene>>, SessionError> {
        if !is_knot_address(&request.address) {
            return Err(SessionError::Unsupported(format!(
                "{} is not a Knot document",
                request.address
            )));
        }
        let opened = self
            .hub
            .open(&request.address)
            .map_err(SessionError::SpawnFailed)?;
        Ok(Box::new(KnotDocumentSession::new(
            self.hub.clone(),
            opened,
            request.viewport,
        )))
    }
}

fn default_endpoint_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate Turnstone executable: {error}"))?;
    let candidate = executable.with_file_name(if cfg!(windows) {
        "knot_endpoint.exe"
    } else {
        "knot_endpoint"
    });
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "TURNSTONE_KNOT_ROOT is set but no knot_endpoint was found beside {}; set TURNSTONE_KNOT_ENDPOINT",
            executable.display()
        ))
    }
}

pub fn is_knot_address(address: &str) -> bool {
    address
        .split(['?', '#'])
        .next()
        .is_some_and(|base| base.to_ascii_lowercase().ends_with(".knot"))
        || address.to_ascii_lowercase().starts_with("knot:")
}

fn run_hub(mut retained: RetainedEndpointSession, commands: Receiver<HubCommand>, wake: Wake) {
    let mut mounted: Option<ProjectionSession> = None;
    let mut bindings = BTreeMap::<String, DocumentBinding>::new();
    let mut subscribers = BTreeMap::<u64, Subscriber>::new();
    loop {
        match commands.recv_timeout(POLL_INTERVAL) {
            Ok(HubCommand::Open {
                registration,
                address,
                events,
                reply,
            }) => {
                let result = ensure_binding(&mut retained, &mut mounted, &mut bindings, &address)
                    .map(|binding| {
                        subscribers.insert(registration, Subscriber { address, events });
                        (registration, binding)
                    });
                let _ = reply.send(result);
            }
            Ok(HubCommand::Save {
                registration,
                base_token,
                source,
            }) => {
                save_from_subscriber(
                    &mut retained,
                    mounted.as_ref(),
                    &mut bindings,
                    &subscribers,
                    registration,
                    base_token,
                    source,
                    &wake,
                );
            }
            Ok(HubCommand::InsertClip {
                address,
                clip,
                status,
            }) => {
                insert_clip(
                    &mut retained,
                    &mut mounted,
                    &mut bindings,
                    &subscribers,
                    &address,
                    clip,
                    &status,
                    &wake,
                );
            }
            Ok(HubCommand::Reload { registration }) => {
                reload_subscriber(
                    &mut retained,
                    mounted.as_ref(),
                    &mut bindings,
                    &subscribers,
                    registration,
                    &wake,
                );
            }
            Ok(HubCommand::Unregister { registration }) => {
                subscribers.remove(&registration);
                if subscribers.is_empty() {
                    if let Some(session) = mounted.take() {
                        retained.forget(&session);
                    }
                    bindings.clear();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if mounted.is_some() {
                    match retained.poll_for_change() {
                        Ok(true) => refresh_subscribers(
                            &mut retained,
                            mounted.as_ref().expect("checked"),
                            &mut bindings,
                            &subscribers,
                            &wake,
                        ),
                        Ok(false) => {}
                        Err(error) => {
                            broadcast(&subscribers, HubEvent::Revoked(error), &wake);
                            break;
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = retained.close();
}

fn ensure_binding(
    retained: &mut RetainedEndpointSession,
    mounted: &mut Option<ProjectionSession>,
    bindings: &mut BTreeMap<String, DocumentBinding>,
    address: &str,
) -> Result<DocumentBinding, String> {
    if let Some(binding) = bindings.get(address) {
        return Ok(binding.clone());
    }
    let session = match mounted {
        Some(session) => session.clone(),
        None => {
            let session = retained.mount(0)?;
            *mounted = Some(session.clone());
            session
        }
    };
    let binding = resolve_binding(retained, &session, address)?;
    bindings.insert(address.to_string(), binding.clone());
    Ok(binding)
}

fn resolve_binding(
    retained: &mut RetainedEndpointSession,
    session: &ProjectionSession,
    address: &str,
) -> Result<DocumentBinding, String> {
    retained
        .resolve_all(session)?
        .into_iter()
        .find_map(|(target, presentation)| binding_from_presentation(target, presentation, address))
        .ok_or_else(|| format!("Knot did not disclose writable text for {address}"))
}

fn binding_from_presentation(
    target: InstanceId,
    presentation: ResolvedPresentation,
    address: &str,
) -> Option<DocumentBinding> {
    let ResolvedContent::EditableText(editable) = presentation.content else {
        return None;
    };
    if editable.address != address {
        return None;
    }
    let save_action = presentation
        .semantics
        .actions
        .iter()
        .find(|action| action.intent.0 == EDITABLE_TEXT_SAVE_INTENT)?
        .clone();
    let clip_action = presentation
        .semantics
        .actions
        .iter()
        .find(|action| action.intent.0 == KNOT_CLIP_INSERT_INTENT)?
        .clone();
    Some(DocumentBinding {
        target,
        save_action,
        clip_action,
        editable,
    })
}

#[allow(clippy::too_many_arguments)]
fn save_from_subscriber(
    retained: &mut RetainedEndpointSession,
    session: Option<&ProjectionSession>,
    bindings: &mut BTreeMap<String, DocumentBinding>,
    subscribers: &BTreeMap<u64, Subscriber>,
    registration: u64,
    base_token: Vec<u8>,
    source: String,
    wake: &Wake,
) {
    let Some(subscriber) = subscribers.get(&registration) else {
        return;
    };
    let Some(session) = session else {
        send_event(
            subscriber,
            HubEvent::Rejected("Knot projection is not mounted".into()),
            wake,
        );
        return;
    };
    let Some(binding) = bindings.get(&subscriber.address).cloned() else {
        send_event(
            subscriber,
            HubEvent::Rejected("Knot document is no longer writable".into()),
            wake,
        );
        return;
    };
    let result = retained.invoke(
        session,
        binding.target,
        &binding.save_action,
        &SaveTextV1 {
            base_token,
            source: source.clone(),
        },
    );
    match result {
        Ok(IntentResult::Accepted) => {
            let refresh = retained
                .wait_for_change()
                .and_then(|_| resolve_binding(retained, session, &subscriber.address));
            match refresh {
                Ok(binding) => {
                    bindings.insert(subscriber.address.clone(), binding.clone());
                    send_event(subscriber, HubEvent::Saved { source, binding }, wake);
                    refresh_subscribers(retained, session, bindings, subscribers, wake);
                }
                Err(error) => send_event(subscriber, HubEvent::Rejected(error), wake),
            }
        }
        Ok(IntentResult::Stale { .. }) => send_event(subscriber, HubEvent::Stale, wake),
        Ok(IntentResult::Rejected { reason }) => {
            send_event(subscriber, HubEvent::Rejected(reason), wake)
        }
        Err(error) => send_event(subscriber, HubEvent::Rejected(error), wake),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_clip(
    retained: &mut RetainedEndpointSession,
    mounted: &mut Option<ProjectionSession>,
    bindings: &mut BTreeMap<String, DocumentBinding>,
    subscribers: &BTreeMap<u64, Subscriber>,
    address: &str,
    clip: PendingClip,
    status: &Arc<Mutex<KnotClipStatus>>,
    wake: &Wake,
) {
    let result = ensure_binding(retained, mounted, bindings, address).and_then(|binding| {
        retained.invoke(
            mounted
                .as_ref()
                .expect("ensure_binding mounted the session"),
            binding.target,
            &binding.clip_action,
            &InsertKnotClipV1 {
                base_token: binding.editable.base_token.clone(),
                source_url: clip.source_url,
                title: clip.title,
                selector: clip.selector,
                knot_body: clip.knot_body,
            },
        )
    });
    let next = match result {
        Ok(IntentResult::Accepted) => retained
            .wait_for_change()
            .and_then(|_| {
                resolve_binding(
                    retained,
                    mounted.as_ref().expect("session remains mounted"),
                    address,
                )
            })
            .map(|current| {
                bindings.insert(address.to_string(), current);
                KnotClipStatus::Saved
            })
            .unwrap_or_else(KnotClipStatus::Rejected),
        Ok(IntentResult::Stale { .. }) => KnotClipStatus::Stale,
        Ok(IntentResult::Rejected { reason }) => KnotClipStatus::Rejected(reason),
        Err(error) => KnotClipStatus::Rejected(error),
    };
    if let Ok(mut current) = status.lock() {
        *current = next;
    }
    if matches!(status.lock().as_deref(), Ok(KnotClipStatus::Saved))
        && let Some(session) = mounted.as_ref()
    {
        refresh_subscribers(retained, session, bindings, subscribers, wake);
    }
    wake();
}

fn reload_subscriber(
    retained: &mut RetainedEndpointSession,
    session: Option<&ProjectionSession>,
    bindings: &mut BTreeMap<String, DocumentBinding>,
    subscribers: &BTreeMap<u64, Subscriber>,
    registration: u64,
    wake: &Wake,
) {
    let Some(subscriber) = subscribers.get(&registration) else {
        return;
    };
    let Some(session) = session else {
        send_event(
            subscriber,
            HubEvent::Rejected("Knot projection is not mounted".into()),
            wake,
        );
        return;
    };
    match resolve_binding(retained, session, &subscriber.address) {
        Ok(binding) => {
            bindings.insert(subscriber.address.clone(), binding.clone());
            send_event(subscriber, HubEvent::Reloaded(binding), wake);
        }
        Err(error) => send_event(subscriber, HubEvent::Revoked(error), wake),
    }
}

fn refresh_subscribers(
    retained: &mut RetainedEndpointSession,
    session: &ProjectionSession,
    bindings: &mut BTreeMap<String, DocumentBinding>,
    subscribers: &BTreeMap<u64, Subscriber>,
    wake: &Wake,
) {
    let addresses = subscribers
        .values()
        .map(|subscriber| subscriber.address.clone())
        .collect::<BTreeSet<_>>();
    for address in addresses {
        match resolve_binding(retained, session, &address) {
            Ok(binding) => {
                bindings.insert(address.clone(), binding.clone());
                for subscriber in subscribers
                    .values()
                    .filter(|subscriber| subscriber.address == address)
                {
                    send_event(subscriber, HubEvent::Remote(binding.clone()), wake);
                }
            }
            Err(error) => {
                bindings.remove(&address);
                for subscriber in subscribers
                    .values()
                    .filter(|subscriber| subscriber.address == address)
                {
                    send_event(subscriber, HubEvent::Revoked(error.clone()), wake);
                }
            }
        }
    }
}

fn broadcast(subscribers: &BTreeMap<u64, Subscriber>, event: HubEvent, wake: &Wake) {
    for subscriber in subscribers.values() {
        send_event(subscriber, event.clone(), wake);
    }
}

fn send_event(subscriber: &Subscriber, event: HubEvent, wake: &Wake) {
    if subscriber.events.send(event).is_ok() {
        wake();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthoringStatus {
    Current,
    Saving,
    Reloading,
    Stale,
    Rejected,
    Revoked,
}

struct AuthoringState {
    editor: KnotEditor,
    status: AuthoringStatus,
    detail: String,
    save_requested: bool,
    reload_requested: bool,
    width: u32,
    height: u32,
}

type AuthoringView = Box<dyn AnyView<AuthoringState, (), GenetCtx, GenetElement>>;
type AuthoringRunner =
    GenetAppRunner<AuthoringState, fn(&AuthoringState) -> AuthoringView, AuthoringView, ()>;

fn authoring_view(state: &AuthoringState) -> AuthoringView {
    let styles = state
        .editor
        .highlights()
        .into_iter()
        .map(|span| StyleRange {
            range: span.range,
            class: format!("syntax-{:?}", span.kind).to_ascii_lowercase(),
        })
        .collect::<Vec<_>>();
    let field = lens(
        move |input: &mut cambium::TextInput| styled_textarea(input, &styles),
        |state: &mut AuthoringState| state.editor.input_mut(),
    );
    let status = match state.status {
        AuthoringStatus::Revoked => "closed".to_string(),
        AuthoringStatus::Stale => "stale; reload or resolve".to_string(),
        AuthoringStatus::Saving => "saving".to_string(),
        AuthoringStatus::Reloading => "reloading".to_string(),
        AuthoringStatus::Rejected => state.detail.clone(),
        AuthoringStatus::Current if state.editor.is_dirty() => "unsaved".to_string(),
        AuthoringStatus::Current => "saved".to_string(),
    };
    let outline = state
        .editor
        .outline()
        .into_iter()
        .map(|item| {
            format!(
                "{}{}",
                "  ".repeat(item.level.saturating_sub(1) as usize),
                item.text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let fold_count = state.editor.folds().len();
    let title = state
        .editor
        .source()
        .lines()
        .next()
        .unwrap_or("Knot document")
        .to_string();
    let preview = state
        .editor
        .preview()
        .map(|document| document.to_markdown())
        .unwrap_or_else(|error| format!("Preview unavailable: {error}"));
    let toolbar = el::<_, AuthoringState, ()>(
        "div",
        (
            el("div", title).attr("class", "knot-title"),
            el("div", status).attr("class", "knot-status"),
            button(
                "Save",
                |state: &mut AuthoringState, _click: PointerClick| {
                    state.save_requested = true;
                },
            )
            .attr("class", "knot-save"),
            button(
                "Reload",
                |state: &mut AuthoringState, _click: PointerClick| {
                    state.reload_requested = true;
                },
            )
            .attr("class", "knot-reload"),
        ),
    )
    .attr("class", "knot-toolbar");
    let editor = el::<_, AuthoringState, ()>("div", field).attr("class", "knot-editor-wrap");
    let readout = el::<_, AuthoringState, ()>(
        "div",
        format!("Outline\n{outline}\n\nFolds: {fold_count}\n\nPreview\n{preview}"),
    )
    .attr("class", "knot-readout");
    Box::new(
        el::<_, AuthoringState, ()>(
            "div",
            (
                toolbar,
                el("div", (editor, readout)).attr("class", "knot-body"),
            ),
        )
        .attr("class", "knot-root")
        .attr(
            "style",
            format!("width: {}px; height: {}px;", state.width, state.height),
        ),
    )
}

/// The visible editor retained in Turnstone's ordinary content-session map.
pub struct KnotDocumentSession {
    hub: Arc<KnotHub>,
    registration: u64,
    address: String,
    base_token: Vec<u8>,
    events: Receiver<HubEvent>,
    revision_refreshes: u64,
    dom: DomHandle,
    runner: AuthoringRunner,
}

impl KnotDocumentSession {
    fn new(hub: Arc<KnotHub>, opened: OpenedDocument, viewport: (u32, u32)) -> Self {
        let address = opened.binding.editable.address.clone();
        let base_token = opened.binding.editable.base_token.clone();
        let dom: DomHandle = Rc::new(std::cell::RefCell::new(ScriptedDom::new()));
        let state = AuthoringState {
            editor: KnotEditor::scratch(&address, opened.binding.editable.source),
            status: AuthoringStatus::Current,
            detail: String::new(),
            save_requested: false,
            reload_requested: false,
            width: viewport.0.max(1),
            height: viewport.1.max(1),
        };
        let runner = AuthoringRunner::new(
            dom.clone(),
            authoring_view as fn(&AuthoringState) -> AuthoringView,
            state,
        );
        Self {
            hub,
            registration: opened.registration,
            address,
            base_token,
            events: opened.events,
            revision_refreshes: 0,
            dom,
            runner,
        }
    }

    pub fn dispatch_key(&mut self, event: KeyEvent) -> bool {
        self.drain_events();
        if matches!(&event.key, Key::Character(value) if event.mods.ctrl && value.eq_ignore_ascii_case("s"))
        {
            self.save();
            return true;
        }
        if self.runner.focus().is_none() {
            return false;
        }
        self.runner.dispatch_key(event);
        true
    }

    pub fn status(&mut self) -> &'static str {
        self.drain_events();
        match self.runner.state().status {
            AuthoringStatus::Current if self.runner.state().editor.is_dirty() => "unsaved",
            AuthoringStatus::Current => "saved",
            AuthoringStatus::Saving => "saving",
            AuthoringStatus::Reloading => "reloading",
            AuthoringStatus::Stale => "stale",
            AuthoringStatus::Rejected => "rejected",
            AuthoringStatus::Revoked => "revoked",
        }
    }

    fn save(&mut self) {
        if matches!(
            self.runner.state().status,
            AuthoringStatus::Saving | AuthoringStatus::Reloading | AuthoringStatus::Revoked
        ) || !self.runner.state().editor.is_dirty()
        {
            return;
        }
        let source = self.runner.state().editor.source().to_string();
        let command = HubCommand::Save {
            registration: self.registration,
            base_token: self.base_token.clone(),
            source,
        };
        if self.hub.commands.send(command).is_ok() {
            self.runner.update(|state| {
                state.status = AuthoringStatus::Saving;
                state.detail.clear();
            });
        } else {
            self.runner.update(|state| {
                state.status = AuthoringStatus::Rejected;
                state.detail = "Knot worker stopped".into();
            });
        }
    }

    fn reload(&mut self) {
        if matches!(
            self.runner.state().status,
            AuthoringStatus::Saving | AuthoringStatus::Reloading | AuthoringStatus::Revoked
        ) {
            return;
        }
        if self
            .hub
            .commands
            .send(HubCommand::Reload {
                registration: self.registration,
            })
            .is_ok()
        {
            self.runner.update(|state| {
                state.status = AuthoringStatus::Reloading;
                state.detail.clear();
            });
        } else {
            self.runner.update(|state| {
                state.status = AuthoringStatus::Rejected;
                state.detail = "Knot worker stopped".into();
            });
        }
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                HubEvent::Remote(binding) => {
                    self.revision_refreshes += 1;
                    if binding.editable.base_token == self.base_token {
                        continue;
                    }
                    if self.runner.state().editor.is_dirty() {
                        self.runner.update(|state| {
                            state.status = AuthoringStatus::Stale;
                            state.detail = "the endpoint has a newer document".into();
                        });
                    } else {
                        self.base_token = binding.editable.base_token;
                        let source = binding.editable.source;
                        let address = self.address.clone();
                        self.runner.update(|state| {
                            state.editor = KnotEditor::scratch(address, source);
                            state.status = AuthoringStatus::Current;
                            state.detail.clear();
                        });
                    }
                }
                HubEvent::Reloaded(binding) => {
                    self.base_token = binding.editable.base_token;
                    let source = binding.editable.source;
                    let address = self.address.clone();
                    self.runner.update(|state| {
                        state.editor = KnotEditor::scratch(address, source);
                        state.status = AuthoringStatus::Current;
                        state.detail.clear();
                    });
                }
                HubEvent::Saved { source, binding } => {
                    self.base_token = binding.editable.base_token;
                    self.runner.update(|state| {
                        state.editor.accept_saved_source(&source);
                        state.status = AuthoringStatus::Current;
                        state.detail.clear();
                    });
                }
                HubEvent::Stale => self.runner.update(|state| {
                    state.status = AuthoringStatus::Stale;
                    state.detail = "the endpoint refused an old base token".into();
                }),
                HubEvent::Rejected(reason) => self.runner.update(|state| {
                    state.status = AuthoringStatus::Rejected;
                    state.detail = reason;
                }),
                HubEvent::Revoked(reason) => {
                    self.base_token.clear();
                    let address = self.address.clone();
                    self.runner.update(|state| {
                        state.editor = KnotEditor::scratch(address, String::new());
                        state.status = AuthoringStatus::Revoked;
                        state.detail = reason;
                    });
                }
            }
        }
    }
}

impl Drop for KnotDocumentSession {
    fn drop(&mut self) {
        let _ = self.hub.commands.send(HubCommand::Unregister {
            registration: self.registration,
        });
    }
}

impl DocumentSession<Scene> for KnotDocumentSession {
    fn frame(&mut self, width: u32, height: u32) -> Scene {
        self.drain_events();
        if (self.runner.state().width, self.runner.state().height) != (width, height) {
            self.runner.update(|state| {
                state.width = width;
                state.height = height;
            });
        }
        let sheet = format!("{} {}", crate::ui::CAMBIUM_SHEET, KNOT_SHEET);
        crate::ui::scene_from_dom(&self.dom.borrow(), &sheet, width, height)
    }

    fn scroll_by(&mut self, _dx: f32, _dy: f32) -> bool {
        false
    }

    fn scroll_for_key(&mut self, _key: SessionScrollKey) -> bool {
        false
    }

    fn click_at(&mut self, x: f32, y: f32) -> SessionClick {
        self.drain_events();
        let size = (self.runner.state().width, self.runner.state().height);
        let hit = {
            let dom = self.dom.borrow();
            let sheet = format!("{} {}", crate::ui::CAMBIUM_SHEET, KNOT_SHEET);
            let layout = IncrementalLayout::new(&*dom, &[&sheet], size.0 as f32, size.1 as f32);
            layout.hit_test(&*dom, x, y, &ScrollOffsets::<NodeId>::default())
        };
        let Some(node) = hit else {
            return SessionClick::Miss;
        };
        let _: Vec<()> = self.runner.dispatch_click(node, PointerClick::at((x, y)));
        if self.runner.state().save_requested {
            self.runner.update(|state| state.save_requested = false);
            self.save();
        }
        if self.runner.state().reload_requested {
            self.runner.update(|state| state.reload_requested = false);
            self.reload();
        }
        SessionClick::Handled
    }

    fn links(&self) -> Vec<SessionLink> {
        Vec::new()
    }

    fn inspect(&self) -> Option<ContentReport> {
        let outline = self
            .runner
            .state()
            .editor
            .outline()
            .into_iter()
            .map(|item| OutlineEntry {
                depth: item.level.saturating_sub(1) as usize,
                role: "heading",
                name: item.text,
            })
            .collect::<Vec<_>>();
        Some(ContentReport {
            title: Some(self.address.clone()),
            headings: outline.iter().map(|entry| entry.name.clone()).collect(),
            outline,
            links: Vec::new(),
        })
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use cambium::{CompositionEvent, Modifiers};
    use layout_dom_api::{LayoutDom, NodeKind};

    use super::*;

    fn file_address(path: &Path) -> String {
        let path = fs::canonicalize(path).expect("test document should canonicalize");
        #[cfg(windows)]
        {
            let path = path.to_string_lossy();
            let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
            format!("file:///{}", path.replace('\\', "/"))
        }
        #[cfg(not(windows))]
        {
            format!("file://{}", path.to_string_lossy())
        }
    }

    fn find_element(dom: &ScriptedDom, node: NodeId, name: &str) -> Option<NodeId> {
        if dom.kind(node) == NodeKind::Element
            && dom
                .element_name(node)
                .is_some_and(|element| element.local.as_ref() == name)
        {
            return Some(node);
        }
        dom.dom_children(node)
            .find_map(|child| find_element(dom, child, name))
    }

    fn focus_editor(session: &mut KnotDocumentSession) {
        let textarea = {
            let dom = session.dom.borrow();
            find_element(&dom, session.runner.root(), "textarea")
                .expect("authoring view should contain a textarea")
        };
        session
            .runner
            .dispatch_click(textarea, PointerClick::at((1.0, 1.0)));
        assert_eq!(session.runner.focus(), Some(textarea));
    }

    fn wait_for_status(session: &mut KnotDocumentSession, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let actual = session.status();
            if actual == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected status {expected}, found {actual}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_refresh(session: &mut KnotDocumentSession, previous: u64) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.revision_refreshes == previous {
            session.drain_events();
            assert!(
                Instant::now() < deadline,
                "endpoint revision bell did not reach the dirty editor"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn type_text(session: &mut KnotDocumentSession, text: &str) {
        assert!(session.dispatch_key(KeyEvent::new(Key::Character(text.into()))));
    }

    fn save_shortcut(session: &mut KnotDocumentSession) {
        assert!(session.dispatch_key(KeyEvent::with_mods(
            Key::Character("s".into()),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )));
    }

    fn wait_for_clip_status(handle: &KnotClipHandle, expected: KnotClipStatus) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let actual = handle.status();
            if actual == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "expected clip status {expected:?}, found {actual:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn knot_addresses_are_selected_without_claiming_other_files() {
        assert!(is_knot_address("file:///C:/notes/one.knot"));
        assert!(is_knot_address("KNOT:vault/document"));
        assert!(is_knot_address("file:///tmp/one.KNOT#heading"));
        assert!(!is_knot_address("file:///tmp/one.md"));
        assert!(!is_knot_address("https://example.test/"));
    }

    #[test]
    #[ignore = "receipt: set KNOT_ENDPOINT_TEST_BIN to a built Mere knot_endpoint"]
    fn real_knot_consumer_saves_rejects_stale_and_reopens() {
        let program = std::env::var_os("KNOT_ENDPOINT_TEST_BIN")
            .expect("KNOT_ENDPOINT_TEST_BIN must name the real Knot endpoint");
        let root =
            std::env::temp_dir().join(format!("turnstone-knot-authoring-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("field.knot");
        fs::write(&path, "# Field\n").unwrap();
        let address = file_address(&path);

        {
            let engine =
                KnotAuthoringEngine::connect_directory(program.clone(), &root, 4096).unwrap();
            let request = SessionSpawnRequest::new(&address).with_viewport(900, 600);
            let mut first = engine.spawn(&request).unwrap();
            let mut second = engine.spawn(&request).unwrap();
            let first = first
                .as_any()
                .downcast_mut::<KnotDocumentSession>()
                .unwrap();
            let second = second
                .as_any()
                .downcast_mut::<KnotDocumentSession>()
                .unwrap();

            focus_editor(first);
            type_text(first, "discard");
            assert!(first.dispatch_key(KeyEvent::with_mods(
                Key::Character("z".into()),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
            )));
            assert_eq!(first.runner.state().editor.source(), "# Field\n");
            type_text(first, "First ");
            let before_preedit = first.runner.state().editor.source().to_string();
            assert!(first.dispatch_key(KeyEvent::new(Key::Composition(
                CompositionEvent::Preedit {
                    text: "かな".into(),
                    selection: Some((3, 3)),
                },
            ))));
            assert_eq!(first.runner.state().editor.source(), before_preedit);
            assert!(
                first.dispatch_key(KeyEvent::new(Key::Composition(CompositionEvent::Commit(
                    "仮名".into()
                ),)))
            );

            focus_editor(second);
            type_text(second, "Second ");
            assert_eq!(second.status(), "unsaved");

            save_shortcut(first);
            wait_for_status(first, "saved");
            wait_for_status(second, "stale");
            assert_eq!(fs::read_to_string(&path).unwrap(), "# Field\nFirst 仮名");

            save_shortcut(second);
            wait_for_status(second, "stale");
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "# Field\nFirst 仮名",
                "the stale local buffer must not overwrite the accepted save"
            );

            second.reload();
            wait_for_status(second, "saved");
            assert_eq!(second.runner.state().editor.source(), "# Field\nFirst 仮名");
            focus_editor(second);
            type_text(second, " after churn");
            let previous_refreshes = second.revision_refreshes;
            fs::write(root.join("other.knot"), "# Other\n").unwrap();
            wait_for_refresh(second, previous_refreshes);
            assert_eq!(
                second.status(),
                "unsaved",
                "an unrelated revision must retain the local buffer"
            );
            save_shortcut(second);
            wait_for_status(second, "saved");
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                "# Field\nFirst 仮名 after churn"
            );
        }

        {
            let engine = KnotAuthoringEngine::connect_directory(program, &root, 4096).unwrap();
            let request = SessionSpawnRequest::new(&address).with_viewport(900, 600);
            let mut reopened = engine.spawn(&request).unwrap();
            let reopened = reopened
                .as_any()
                .downcast_mut::<KnotDocumentSession>()
                .unwrap();
            assert_eq!(
                reopened.runner.state().editor.source(),
                "# Field\nFirst 仮名 after churn"
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "receipt: set KNOT_ENDPOINT_TEST_BIN to a built Mere knot_endpoint"]
    fn inspector_clip_crosses_the_typed_endpoint_action() {
        let program = std::env::var_os("KNOT_ENDPOINT_TEST_BIN")
            .expect("KNOT_ENDPOINT_TEST_BIN must name the real Knot endpoint");
        let root =
            std::env::temp_dir().join(format!("turnstone-knot-clip-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("field.knot");
        fs::write(&path, "# Field\n").unwrap();
        let address = file_address(&path);

        let engine = KnotAuthoringEngine::connect_directory(program, &root, 4096).unwrap();
        let handle = KnotClipHandle {
            hub: engine.hub.clone(),
            target: address,
            status: Arc::new(Mutex::new(KnotClipStatus::Ready)),
        };
        handle
            .insert(inker::DocumentClip {
                source_url: "https://example.test/report".into(),
                title: Some("The report".into()),
                text: "A useful finding.".into(),
                selector: None,
                links: vec!["https://example.test/source".into()],
            })
            .unwrap();
        wait_for_clip_status(&handle, KnotClipStatus::Saved);

        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains("```knot.clip.provenance"));
        assert!(saved.contains(r#""source_url":"https://example.test/report""#));
        assert!(saved.contains("# The report"));
        assert!(saved.contains("A useful finding."));
        assert!(saved.contains("https://example.test/source"));

        fs::remove_dir_all(root).unwrap();
    }
}
