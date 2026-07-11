//! The one vocabulary: everything that acts on merecat lowers to an
//! [`Action`]; everything slow or platform-shaped leaves [`crate::app`] as an
//! [`Effect`] the shell runs through a port; services answer with an
//! [`Update`] drained on wake. Settings, automation, scenarios, scripting,
//! and remote control all speak this vocabulary, so no lane grows a second
//! execution model (the architecture plan's doctrine 2 — the meerkat
//! `command_drain` lesson).
//!
//! Two deliberate boundaries:
//!
//! * **The gesture law.** Ephemeral interaction may bypass Action: the
//!   canvas's semantic input methods (`pointer_down`, `cursor_moved`,
//!   `wheel`, ...) are already a typed vocabulary at the right granularity,
//!   and the shell maps raw input onto them directly. Durable or externally
//!   observable semantic change may not bypass — a gesture that ends in one
//!   surfaces a semantic event at gesture end. `Action` is the app-intent
//!   tier (navigate, reseed, flip a view mode), the tier automation and
//!   commands speak.
//! * **Port-agnostic messages.** This module never imports a service crate:
//!   [`Update`] carries app-owned types, and each port's adapter
//!   ([`crate::browse`] for the fetch actor) converts the service's concrete
//!   types at the boundary. The universal vocabulary must not depend on one
//!   port implementation.

/// A typed app intent. The shell (keys, later the omnibar / command palette /
/// automation adapters) produces these; [`crate::app::update`] consumes them.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Open an address: mint/select its node in the graph and fetch it.
    OpenAddress(String),
    /// Re-seed the canvas layout and replay the settle.
    ReseedLayout,
    /// Toggle the isometric (2.5D foreshortened) view.
    ToggleIsometric,
    /// Orbit the view (yaw) by radians.
    OrbitBy(f32),
    /// Tilt the view (vertical foreshorten) by a delta.
    TiltBy(f32),
    /// Toggle height-by-degree (hubs float above the ground plane).
    ToggleHeightByDegree,
    /// Persist the session now (close path; enrichment saves ride effects).
    SaveSession,
    /// Flip the focused node's live content: spawn a document session for it
    /// through the content port, or close the one it has (rung 4; the
    /// session-engines plan's phase-4 consumer intent).
    ToggleNodeContent,
    /// Summon the omnibar (`command` pre-seeds the `>` actions lane).
    OmnibarOpen { command: bool },
    /// Dismiss the omnibar without committing.
    OmnibarClose,
    /// Insert one typed character at the caret.
    OmnibarChar(char),
    /// Insert a string at the caret (an IME commit; later, paste).
    OmnibarInsert(String),
    /// Delete the character before the caret.
    OmnibarBackspace,
    /// Delete the character after the caret.
    OmnibarDelete,
    /// Move the caret within the omnibar text.
    OmnibarCaret(CaretMove),
    /// Move the suggestion highlight by a delta (wraps at the ends).
    OmnibarMove(i32),
    /// Commit the highlighted suggestion (or literal-go on address-shaped
    /// text with nothing highlighted).
    OmnibarCommit,
}

/// A caret movement within the omnibar's single line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaretMove {
    Left,
    Right,
    Home,
    End,
}

/// The palette's action registry: every Action an app-intent lane (the `>`
/// omnibar lane today; automation and a context menu later) may offer, with
/// its display label. The registry is the single catalog those lanes filter;
/// an Action absent here is reachable only by its dedicated input path.
pub fn palette_actions() -> Vec<(&'static str, Action)> {
    vec![
        ("Reseed layout", Action::ReseedLayout),
        ("Toggle isometric view", Action::ToggleIsometric),
        ("Toggle height-by-degree", Action::ToggleHeightByDegree),
        ("Orbit left", Action::OrbitBy(-0.15)),
        ("Orbit right", Action::OrbitBy(0.15)),
        ("Toggle live content", Action::ToggleNodeContent),
        ("Save session", Action::SaveSession),
    ]
}

/// A side effect `update` asks the shell to run through a port. `update`
/// itself never blocks and never touches a platform API.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Fetch a page document through the fetch actor.
    FetchPage(String),
    /// Fetch a favicon (already-absolute `url`) for the node at `owner_url`.
    FetchFavicon { owner_url: String, url: String },
    /// Persist the session through the persistence port.
    SaveSession,
    /// Spawn a live document session for `node` at `url` through the
    /// content port (registry-dispatched once serval-documents lands;
    /// until then the port answers with an honest ContentFailed).
    SpawnContent { node: uuid::Uuid, url: String },
    /// Close `node`'s live session; the port drops the handle.
    CloseContent { node: uuid::Uuid },
    /// The projection is stale; present another frame.
    Redraw,
}

/// A typed service answer, drained by the shell on wake and folded back into
/// state through [`crate::app::apply_update`]. App-owned types only; port
/// adapters convert.
pub enum Update {
    /// A page fetch completed (successfully or not).
    PageFetched {
        url: String,
        result: Result<FetchedPage, String>,
    },
    /// A favicon's raw bytes arrived for the node at `owner_url`.
    FaviconFetched { owner_url: String, bytes: Vec<u8> },
    /// The content port spawned a live session for `node`.
    ContentSpawned { node: uuid::Uuid },
    /// The content port could not spawn (or lost) `node`'s session.
    ContentFailed { node: uuid::Uuid, error: String },
}

/// A successfully fetched page document, in app-owned terms.
pub struct FetchedPage {
    /// The response's Content-Type header, verbatim.
    pub content_type: Option<String>,
    /// The decoded body text.
    pub body: String,
}
