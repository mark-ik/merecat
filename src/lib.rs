//! Turnstone application core and desktop host.
//!
//! The library boundary exposes the existing read model, action reducer, and
//! remote projection adapter. Platform handles remain in [`shell`].

pub mod a11y;
pub mod action;
pub mod app;
mod apparatus_pane;
mod browse;
mod cambium_pane;
mod chrome_view;
#[cfg(feature = "wasm")]
mod component;
mod content;
mod content_classes;
mod denizen;
mod identity;
mod inspector_pane;
mod inspector_view;
mod knot_authoring;
pub mod observe;
mod overmap;
mod pane;
mod publish_pane;
mod publish_service;
pub mod place;
/// Turnstone's own pane model (was the `frisket` crate; folded in 2026-07-25).
pub mod panes;
mod recycle;
pub mod remote_projection;
mod ring;
mod roster_view;
mod scenario;
#[cfg(feature = "piccolo")]
mod script;
mod sections;
mod session;
pub mod settings_provider;
mod settings_pane;
pub mod shell;
mod surface;
mod swatch_pane;
mod trail_pane;
mod trail_view;
mod ui;
mod workbench_pane;
mod workbench_tiling;
