//! Merecat: a graph-workspace browser and the reference host for the mere
//! library.
//!
//! Architecture (design_docs/2026-07-10_merecat_architecture_plan.md): one
//! typed vocabulary. Platform events lower to [`action::Action`]s;
//! [`app::App::update`] mutates state and returns [`action::Effect`]s; the
//! [`shell::Shell`] runs effects through ports (the fetch and physics actors,
//! the persistence store) and folds their typed answers back through
//! [`app::App::apply_update`]. Continuous canvas gestures map onto
//! `mere::canvas`'s semantic input methods directly — the canvas is hosted,
//! not wrapped.
//!
//! Run with an address to open it (the graph remembers across launches), or
//! bare to restore the last session:
//!
//! ```text
//! cargo run -- https://example.com
//! ```
//!
//! Navigation (per the graph-canvas defaults): wheel = pan, Ctrl+wheel =
//! cursor-anchored zoom, middle-drag = pan, all with inertia. Left-drag grabs
//! and pins the node under the cursor; a click selects; a drag on empty space
//! marquee-selects; a bare empty click clears. Space re-seeds the layout;
//! `i` toggles the isometric view, `q`/`e` orbit, `[`/`]` tilt, `h` toggles
//! height-by-degree.

mod a11y;
mod action;
mod app;
mod apparatus_pane;
mod browse;
mod cambium_pane;
mod chrome_view;
mod content;
mod gloss_pane;
mod inspector_pane;
mod inspector_view;
mod trail_pane;
mod observe;
mod overmap;
mod overmap_pane;
mod pane;
mod roster_view;
mod scenario;
#[cfg(feature = "piccolo")]
mod script;
mod session;
mod shell;
mod surface;
mod trail_view;
mod ui;
mod workbench_pane;
mod workbench_tiling;

use winit::event_loop::EventLoop;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("merecat=info")),
        )
        .init();

    // Which graph actually shows (restored session / fresh-from-address /
    // sample) is decided and logged inside `App::boot`, after the restore
    // attempt; claiming it here would lie on a restoring launch.
    let address = std::env::args().nth(1);
    match &address {
        Some(url) => tracing::info!(%url, "merecat starting on an address"),
        None => tracing::info!("merecat starting"),
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut shell = shell::Shell::new(proxy, address);
    event_loop.run_app(&mut shell).expect("event loop error");
}
