//! Merecat: a graph-workspace browser and the reference host for the mere
//! library.
//!
//! First vertical slices (boundary pass follow-on, 2026-07-09): open address
//! -> mere graph node -> visible canvas, then the first breath of the web
//! lane: the address FETCHES (mere's fetch actor on armillary), the page's
//! `<title>` and Content-Type enrich the node, and the canvas caption flips
//! from the host fallback to the real title. A thin winit shell hosts the
//! window-agnostic `mere::canvas::Canvas` content-root — the same content-root
//! meerkat hosts in-workspace — proving the founding doc's first
//! done-condition: merecat builds and runs from this repo against mere as a
//! dependency. The full browser runtime (verso lane), chrome, panes, and
//! session land as later slices; nothing here is copied from meerkat's shell.
//!
//! Run with an address to seed the graph from it, or bare for the sample
//! graph:
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

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use fetch::{FetchCommand, FetchOutcome, FetchUpdate};
use mere::canvas::{Canvas, PointerButton, WHEEL_PAN_SCALE};
use session_runtime::session_graph_store;
use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions};
use serval_winit_host::SurfaceHost;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WinitKey, NamedKey as WinitNamedKey};
use winit::window::{Window, WindowId};

/// The merecat shell: the reusable canvas content-root plus the window and
/// present stack that drive it.
struct App {
    canvas: Canvas,
    /// The per-user data root; the session graph persists at its flat
    /// `graph.json` (the single-session shape; sessions/<id>/ arrives with
    /// multi-session later).
    data_root: PathBuf,
    /// Wakes the loop when the physics or fetch actor has news.
    proxy: EventLoopProxy<()>,
    /// The fetch actor's command handle; dropping it ends the actor.
    fetch_handle: armillary::ActorHandle<FetchCommand>,
    /// Completed fetches, drained in `user_event` on each wake.
    fetch_rx: Receiver<FetchUpdate>,
    /// Last cursor position in physical px. winit's `MouseInput` carries no
    /// position, so the shell tracks it from `CursorMoved`.
    cursor: (f32, f32),
    window: Option<Arc<Window>>,
    host: Option<SurfaceHost>,
    width: u32,
    height: u32,
}

impl App {
    fn new(proxy: EventLoopProxy<()>, address: Option<String>) -> Self {
        // The browser remembers: a persisted session graph (titles, favicons,
        // relations) restores first. Failing that, open-address seeds a fresh
        // graph; a bare first launch shows the sample graph so the canvas is
        // never empty (it persists like anything else; delete graph.json for
        // a clean profile, or point MERECAT_ROOT at a scratch one).
        let data_root = default_merecat_root();
        let _ = std::fs::create_dir_all(&data_root);
        let graph_file = data_root.join(session_graph_store::GRAPH_FILE);
        let restored = match session_graph_store::load(&graph_file) {
            Ok(graph) => graph,
            Err(err) => {
                tracing::warn!(%err, path = ?graph_file, "failed to load the session graph; starting fresh");
                None
            }
        };
        let mut canvas = match (restored, &address) {
            (Some(graph), _) => {
                tracing::info!(path = ?graph_file, "session graph restored");
                Canvas::with_graph(graph)
            }
            (None, Some(url)) => {
                tracing::info!(%url, "fresh graph seeded from the address");
                Canvas::new()
            }
            (None, None) => {
                tracing::info!("no session graph; starting on the sample graph");
                Canvas::with_sample_graph()
            }
        };
        // The web lane's first breath: the fetch actor on its own armillary
        // thread, waking this loop like the physics actor does. The seed
        // address fetches immediately; its outcome enriches the node.
        let fetch_proxy = proxy.clone();
        let fetch_wake: armillary::Wake = Arc::new(move || {
            let _ = fetch_proxy.send_event(());
        });
        let (fetch_handle, fetch_rx) = fetch::spawn_fetcher(fetch_wake);
        if let Some(url) = &address {
            canvas.visit(url);
            if fetch::is_fetchable(url) {
                fetch_handle.command(FetchCommand::Page(url.clone()));
            }
        }
        Self {
            canvas,
            data_root,
            proxy,
            fetch_handle,
            fetch_rx,
            cursor: (0.0, 0.0),
            window: None,
            host: None,
            width: 1024,
            height: 600,
        }
    }

    /// Persist the session graph at the flat `graph.json`. Best-effort: a
    /// write failure is logged, not fatal. Called after each enrichment (so a
    /// crash loses nothing) and on close.
    fn save_session(&self) {
        let graph_file = self.data_root.join(session_graph_store::GRAPH_FILE);
        if let Err(err) = session_graph_store::save(&graph_file, self.canvas.graph()) {
            tracing::warn!(%err, path = ?graph_file, "failed to persist the session graph");
        }
    }

    /// Fold one completed page fetch into the graph: stamp the response's
    /// Content-Type as the node's MIME hint, and for HTML extract the page
    /// `<title>` (render-free static parse) so the canvas caption flips from
    /// the host fallback to the real title, then chase the page's favicon so
    /// the node face wears a real icon.
    fn apply_page_outcome(&mut self, outcome: FetchOutcome) {
        let url = outcome.url;
        match outcome.result {
            Ok(fetched) => {
                let media = fetched
                    .content_type
                    .as_deref()
                    .and_then(|ct| ct.split(';').next())
                    .map(|m| m.trim().to_ascii_lowercase());
                tracing::info!(%url, content_type = ?media, bytes = fetched.body.len(), "page fetched");
                self.canvas.set_node_mime_hint(&url, media.clone());
                if media.as_deref() == Some("text/html") {
                    let doc = serval_static_dom::StaticDocument::parse(&fetched.body);
                    if let Some(title) = serval_extract::extract(&doc).title {
                        if self.canvas.set_node_title(&url, title.clone()) {
                            tracing::info!(%url, %title, "node title enriched from the page");
                        }
                    }
                    // Best-effort: fetch the page's favicon; the bytes route
                    // back as FetchUpdate::Favicon keyed to this page url.
                    if let Some(icon_url) = favicon_url_for(&url, &doc) {
                        self.fetch_handle.command(FetchCommand::Favicon {
                            owner_url: url.clone(),
                            url: icon_url,
                        });
                    }
                }
            }
            Err(err) => {
                tracing::warn!(%url, %err, "page fetch failed");
            }
        }
        self.save_session();
        self.request_redraw();
    }

    /// A page's favicon arrived: decode it to RGBA and stamp it on the node
    /// currently at the owner url; the canvas paints it on that node's face
    /// on the next frame.
    fn apply_favicon(&mut self, owner_url: &str, bytes: &[u8]) {
        if let Some(decoded) = serval_layout::decode_image_bytes(bytes) {
            if self
                .canvas
                .set_node_favicon(owner_url, decoded.rgba, decoded.width, decoded.height)
            {
                tracing::info!(url = %owner_url, "node favicon enriched from the page");
                self.save_session();
                self.request_redraw();
            }
        }
    }

    /// Produce the canvas's frame at the current size, rasterize + composite
    /// it through the present stack, and chain another redraw while the canvas
    /// is still animating (settling / gliding / dragging).
    fn render(&mut self) {
        if self.host.is_none() {
            return;
        }
        let (w, h) = (self.width.max(1), self.height.max(1));
        let (scene, needs_redraw) = self.canvas.frame(w, h);

        let host = self.host.as_ref().unwrap();
        let (_tex, view) = host.rasterize(&scene, w, h, ColorLoad::Clear(wgpu::Color::WHITE));
        let Some(frame) = host.acquire() else { return };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        host.renderer().compose_external_texture(
            &view,
            &target,
            host.format(),
            w,
            h,
            ExternalTexturePlacement::new([0.0, 0.0, w as f32, h as f32]),
        );
        frame.present();

        if needs_redraw {
            self.request_redraw();
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Merecat")
            .with_inner_size(PhysicalSize::new(self.width, self.height));
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .expect("failed to create the merecat window"),
        );
        let size = window.inner_size();
        self.width = size.width.max(1);
        self.height = size.height.max(1);
        self.canvas.resize(self.width, self.height);
        self.canvas.recenter();

        let options = NetrenderOptions {
            tile_cache_size: Some(64),
            enable_vello: true,
            ..Default::default()
        };
        match SurfaceHost::boot(window.clone(), self.width, self.height, options) {
            Ok(host) => self.host = Some(host),
            Err(err) => {
                eprintln!("[merecat] {err}");
                event_loop.exit();
                return;
            }
        }

        // Always-offload physics: the simulation runs on an armillary actor
        // thread and wakes this loop through the proxy when a layout snapshot
        // lands, so a heavy settle never blocks compositing or input.
        let proxy = self.proxy.clone();
        let physics_wake: armillary::Wake = Arc::new(move || {
            let _ = proxy.send_event(());
        });
        self.canvas.offload_physics(physics_wake);

        window.request_redraw();
        self.window = Some(window);
    }

    /// An actor woke us through the proxy: a physics layout snapshot or a
    /// completed fetch is waiting. Drain fetches into the graph, then redraw
    /// so `frame()` folds everything in (and chains while settling).
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        while let Ok(update) = self.fetch_rx.try_recv() {
            match update {
                FetchUpdate::Page(outcome) => self.apply_page_outcome(outcome),
                FetchUpdate::Favicon { owner_url, bytes } => {
                    self.apply_favicon(&owner_url, &bytes)
                }
                // Subresources arrive with the content lane in a later slice.
                FetchUpdate::Subresource(_) => {}
            }
        }
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if self.window.as_ref().map(|w| w.id()) != Some(window_id) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.save_session();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.width = size.width.max(1);
                self.height = size.height.max(1);
                if let Some(host) = self.host.as_mut() {
                    host.resize(self.width, self.height);
                }
                self.canvas.resize(self.width, self.height);
                self.request_redraw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                self.canvas.set_ctrl(mods.state().control_key());
                self.canvas.set_alt(mods.state().alt_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                if self.canvas.cursor_moved(self.cursor.0, self.cursor.1) {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                if self.canvas.wheel(dx, dy) {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Some(PointerButton::Left),
                    MouseButton::Middle => Some(PointerButton::Middle),
                    MouseButton::Right => Some(PointerButton::Right),
                    _ => None,
                };
                if let Some(button) = button {
                    let (x, y) = self.cursor;
                    let redraw = match state {
                        ElementState::Pressed => self.canvas.pointer_down(button, x, y),
                        ElementState::Released => self.canvas.pointer_up(button, x, y),
                    };
                    if redraw {
                        self.request_redraw();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match &event.logical_key {
                        // Space re-seeds the layout and replays the settle.
                        WinitKey::Named(WinitNamedKey::Space) => {
                            if self.canvas.reseed() {
                                self.request_redraw();
                            }
                        }
                        // `i` toggles the isometric (2.5D foreshortened) view.
                        WinitKey::Character(s) if s.as_str() == "i" => {
                            let on = !self.canvas.is_isometric();
                            self.canvas.set_isometric(on);
                            self.request_redraw();
                        }
                        // `q` / `e` orbit the view (yaw).
                        WinitKey::Character(s) if s.as_str() == "q" => {
                            self.canvas.orbit_by(-0.15);
                            self.request_redraw();
                        }
                        WinitKey::Character(s) if s.as_str() == "e" => {
                            self.canvas.orbit_by(0.15);
                            self.request_redraw();
                        }
                        // `[` / `]` sweep the vertical foreshorten (tilt).
                        WinitKey::Character(s) if s.as_str() == "[" => {
                            self.canvas.set_tilt(self.canvas.tilt() - 0.05);
                            self.request_redraw();
                        }
                        WinitKey::Character(s) if s.as_str() == "]" => {
                            self.canvas.set_tilt(self.canvas.tilt() + 0.05);
                            self.request_redraw();
                        }
                        // `h` toggles height-by-degree (hubs float above ground).
                        WinitKey::Character(s) if s.as_str() == "h" => {
                            let on = !self.canvas.height_by_degree();
                            self.canvas.set_height_by_degree(on);
                            self.request_redraw();
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

/// The per-user data root (`<data_dir>/merecat`). A `MERECAT_ROOT` override
/// points the whole root at a scratch profile, so a headed-verification run
/// (or any throwaway session) isolates from the real per-user data dir (the
/// meerkat `MERE_ROOT` convention).
fn default_merecat_root() -> PathBuf {
    if let Some(root) = std::env::var_os("MERECAT_ROOT") {
        return PathBuf::from(root);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("merecat")
}

/// The favicon URL for a fetched page: the document's declared
/// `<link rel=icon>` href resolved against the page URL, else the well-known
/// `/favicon.ico` for web pages. `None` when neither applies.
fn favicon_url_for(page_url: &str, doc: &serval_static_dom::StaticDocument) -> Option<String> {
    let base = url::Url::parse(page_url).ok()?;
    if let Some(href) = serval_layout::linked_icon_href(doc) {
        if let Ok(resolved) = base.join(&href) {
            return Some(resolved.to_string());
        }
    }
    if matches!(base.scheme(), "http" | "https") {
        if let Ok(fallback) = base.join("/favicon.ico") {
            return Some(fallback.to_string());
        }
    }
    None
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("merecat=info")),
        )
        .init();

    // Which graph actually shows (restored session / fresh-from-address /
    // sample) is decided and logged inside `App::new`, after the restore
    // attempt; claiming it here would lie on a restoring launch.
    let address = std::env::args().nth(1);
    match &address {
        Some(url) => tracing::info!(%url, "merecat starting on an address"),
        None => tracing::info!("merecat starting"),
    }

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App::new(proxy, address);
    event_loop.run_app(&mut app).expect("event loop error");
}
