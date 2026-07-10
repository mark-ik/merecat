//! The one vocabulary: everything that acts on merecat lowers to an
//! [`Action`]; everything slow or platform-shaped leaves [`crate::app`] as an
//! [`Effect`] the shell runs through a port; services answer with an
//! [`Update`] drained on wake. Settings, automation, scenarios, scripting,
//! and remote control all speak this vocabulary, so no lane grows a second
//! execution model (the architecture plan's doctrine 2 — the meerkat
//! `command_drain` lesson).
//!
//! One deliberate boundary: **continuous canvas gestures are not Actions.**
//! The canvas's semantic input methods (`pointer_down`, `cursor_moved`,
//! `wheel`, ...) are already a typed vocabulary at the right granularity;
//! the shell maps raw input onto them directly. `Action` is the app-intent
//! tier above (navigate, reseed, flip a view mode), the tier automation and
//! commands speak.

use fetch::FetchUpdate;

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
    /// The projection is stale; present another frame.
    Redraw,
}

/// A typed service answer, drained by the shell on wake and folded back into
/// state through [`crate::app::apply_update`].
pub enum Update {
    /// The fetch actor completed a page / favicon fetch.
    Fetch(FetchUpdate),
}
