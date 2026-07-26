//! Lens windows: a second window as a pane host over the one app state.
//!
//! A lens owns its own camera (installed around each pass and stashed back)
//! and its own frisket space, so panes torn out of the primary keep their
//! retained runners and their identity. Arrangement rides the SESSION, so
//! adopting a session closes these and reopens that session's own.

use std::sync::Arc;

use netrender::external_texture::ExternalTexturePlacement;
use netrender::{ColorLoad, NetrenderOptions, Scene};
use genet_winit_host::SurfaceHost;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

use mere::canvas::WHEEL_PAN_SCALE;

use crate::action::Action;
use crate::panes::PaneContent;
use crate::surface::{Rect, SurfaceKind};

use inker::SessionClick;

use super::{CompositeLayer, PlannedScene, Shell, capture_composed};
use super::input::pointer_button;


/// One lens window's record: its platform window, present stack, size,
/// cursor, and camera.
pub(super) struct LensWindow {
    pub(super) window: Arc<Window>,
    pub(super) host: SurfaceHost,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) cursor: (f32, f32),
    pub(super) viewport: mere::canvas::Viewport,
    /// Whether a canvas pointer gesture (grab/pan) is in flight here.
    pub(super) pointer_down: bool,
    /// Which `App::lenses` pane space this window shows (stable; the space
    /// tombstones on close).
    pub(super) ordinal: usize,
}

impl Shell {
    /// Create any requested lens windows (rung 7). Called from the event
    /// handlers, where an `ActiveEventLoop` is in scope. A lens seeds its
    /// camera from the canvas's CURRENT viewport (sensible initial framing),
    /// then owns it.
    pub(super) fn drain_pending_windows(&mut self, event_loop: &ActiveEventLoop) {
        while let Some(ordinal) = self.pending_windows.pop() {
            let attributes = Window::default_attributes()
                .with_title("Turnstone — lens")
                .with_inner_size(PhysicalSize::new(800u32, 600u32));
            let Ok(window) = event_loop.create_window(attributes) else {
                tracing::warn!("lens window creation failed");
                continue;
            };
            let window = Arc::new(window);
            let size = window.inner_size();
            let options = NetrenderOptions {
                tile_cache_size: Some(16),
                enable_vello: true,
                ..Default::default()
            };
            match SurfaceHost::boot(window.clone(), size.width.max(1), size.height.max(1), options)
            {
                Ok(host) => {
                    window.request_redraw();
                    self.lens_windows.insert(
                        window.id(),
                        LensWindow {
                            window,
                            host,
                            width: size.width.max(1),
                            height: size.height.max(1),
                            cursor: (0.0, 0.0),
                            viewport: self.app.canvas.viewport(),
                            pointer_down: false,
                            ordinal,
                        },
                    );
                }
                Err(err) => tracing::warn!(%err, "lens surface boot failed"),
            }
        }
        self.app.window_count = 1 + self.lens_windows.len();
    }

    /// A lens window's surface plan: its OWN pane space (`App::lenses`) walked
    /// at its size — the same geometry the primary uses, per window. Canvas
    /// leaf = the lens camera's view; other leaves = panes; seams = dividers;
    /// a torn-out workbench's live tiles = content surfaces. No canvas-inset
    /// content in a lens (the tile IS the lens's content story); chrome
    /// composites separately in `render_lens`.
    pub(super) fn lens_plan(&self, ordinal: usize, w: u32, h: u32) -> Vec<crate::surface::Surface> {
        let Some(Some(space)) = self.app.lenses.get(ordinal) else {
            return Vec::new();
        };
        let area = Rect::full(w.max(1), h.max(1));
        let tiling = crate::pane::place_panes(space, area, None);
        let mut base: Vec<(SurfaceKind, Rect)> = tiling
            .panes
            .iter()
            .map(|p| {
                if matches!(p.content, PaneContent::Orrery) {
                    (SurfaceKind::Canvas, p.rect)
                } else if let PaneContent::Tile(m) = p.content
                    && self.content_sessions.contains_key(&m)
                {
                    // A torn-out tile: the pinned pane composites its live
                    // session as this window's content surface.
                    (SurfaceKind::Content(m), p.rect)
                } else {
                    (SurfaceKind::Pane(p.id), p.rect)
                }
            })
            .collect();
        base.extend(
            tiling
                .dividers
                .iter()
                .map(|d| (SurfaceKind::Divider(d.index), d.rect)),
        );
        // Workbench tiles in a LENS (rung-7 depth: content tiles follow the
        // pane): when the workbench pane tore out to this window, its cells'
        // live tiles compose as content surfaces at their body rects — the
        // same walk the primary plan does, at the lens pane's rect.
        let wb_rect = tiling
            .panes
            .iter()
            .find(|p| matches!(p.content, PaneContent::Workbench))
            .map(|p| p.rect);
        let tiles: Vec<(uuid::Uuid, Rect)> = wb_rect
            .map(|rect| {
                let geom = self.app.workbench.to_arrangement().1;
                crate::workbench_tiling::place_workbench(geom.as_ref(), rect)
                    .cells
                    .iter()
                    .filter_map(|c| {
                        let m = c.active_member()?;
                        self.content_sessions.contains_key(&m).then(|| (m, c.body()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        crate::surface::assemble(&base, &tiles, None, None)
    }

    /// A pane's `PaneContent` in a LENS window's space.
    pub(super) fn lens_pane_content(&self, ordinal: usize, id: crate::panes::PaneId) -> Option<PaneContent> {
        self.app
            .lenses
            .get(ordinal)
            .and_then(|s| s.as_ref())
            .and_then(|space| {
                space
                    .iter_leaves()
                    .find(|(pid, _, _)| *pid == id)
                    .map(|(_, content, _)| content.clone())
            })
    }

    /// Render one lens window: its pane space composited through its host —
    /// the canvas leaf through the lens camera (installed around the frame,
    /// stashed after), every other leaf through the SAME retained pane runner
    /// the primary uses. That shared runner is the identity story: a pane torn
    /// out to a lens keeps its DOM, widget state, and scroll because the
    /// runner never moved — only its leaf changed trees.
    pub(super) fn render_lens(&mut self, id: WindowId) {
        let Some(lens) = self.lens_windows.get(&id) else {
            return;
        };
        let (lw, lh, ordinal, lens_viewport) =
            (lens.width, lens.height, lens.ordinal, lens.viewport);
        let surfaces = self.lens_plan(ordinal, lw, lh);
        if surfaces.is_empty() {
            return;
        }
        // Pass 1 (mutable): produce each surface's scene at its rect size.
        // Sessions pump here too (the bug-#2 discipline): a lens hosting the
        // workbench must keep its tiles' clocks honest even while the primary
        // idles. The pump clock is shared and monotonic, so double-pumping in
        // a frame where both windows render is a no-op.
        let now_ms = self.epoch.elapsed().as_secs_f64() * 1000.0;
        let mut animating = false;
        for session in self.content_sessions.values_mut() {
            session.pump(now_ms);
            if !session.settled() {
                animating = true;
            }
        }
        let mut new_viewport = lens_viewport;
        let mut scenes: Vec<PlannedScene> = Vec::with_capacity(surfaces.len());
        for surface in &surfaces {
            let rect = surface.rect;
            let (rw, rh) = (rect.w.round().max(1.0) as u32, rect.h.round().max(1.0) as u32);
            let (scene, clear) = match surface.kind {
                crate::surface::SurfaceKind::Canvas => {
                    // Install this lens's camera, frame, stash it back, restore
                    // the primary's. A `Viewport` carries the view size it was
                    // framed for, so installing one sets the size WITH the
                    // camera and no `resize` is involved — `resize` re-centres,
                    // which is right for a window that actually changed size and
                    // wrong for a camera we are only borrowing (it drifted both
                    // windows off-screen, mere-canvas pins the invariant).
                    let saved = self.app.canvas.viewport();
                    self.app.canvas.set_viewport(new_viewport);
                    self.app.drive_layout_strategy(rw, rh);
                    let (scene, anim) = self.app.canvas.frame(rw, rh);
                    animating |= anim;
                    new_viewport = self.app.canvas.viewport();
                    self.app.canvas.set_viewport(saved);
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Pane(pid) => {
                    let content = self.lens_pane_content(ordinal, pid);
                    let scene = self.pane_scene_by_kind(content.as_ref(), rw, rh);
                    (scene, wgpu::Color::TRANSPARENT)
                }
                // A workbench tile whose pane tore out here: the SAME session
                // the primary would frame, at this cell's size (already pumped
                // above).
                crate::surface::SurfaceKind::Content(node) => {
                    let Some(session) = self.content_sessions.get_mut(&node) else {
                        continue;
                    };
                    let scene = session.frame(rw, rh);
                    (scene, wgpu::Color::WHITE)
                }
                crate::surface::SurfaceKind::Divider(_) => {
                    (Scene::default(), crate::ui::SEAM_CLEAR)
                }
                // No canvas-inset content / chrome layer in a lens's plan
                // (chrome composites separately below).
                _ => continue,
            };
            scenes.push(PlannedScene {
                id: surface.id.0,
                kind: surface.kind,
                placement: ExternalTexturePlacement::new(rect.dest()),
                dims: (rw, rh),
                scene,
                clear,
            });
        }
        // The lens's chrome (its window-root in the shared chrome forest):
        // the caption chip, composited on top when there is one to show.
        if crate::app::focused_caption(&self.app.canvas).is_some() {
            let slot = ordinal + 1;
            self.chrome.ensure_slot(slot);
            let scene = self.chrome.scene(slot, lw, lh);
            scenes.push(PlannedScene {
                id: crate::surface::SurfaceId::CHROME.0,
                kind: crate::surface::SurfaceKind::Chrome,
                placement: ExternalTexturePlacement::new([0.0, 0.0, lw as f32, lh as f32]),
                dims: (lw, lh),
                scene,
                clear: wgpu::Color::TRANSPARENT,
            });
        }
        // Pass 2 (immutable host): rasterize + compose, keyed per surface.
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        lens.viewport = new_viewport;
        let layers: Vec<CompositeLayer> = scenes
            .iter()
            .map(|s| {
                let (_tex, view) = lens.host.core().rasterize_for(
                    s.id,
                    &s.scene,
                    s.dims.0,
                    s.dims.1,
                    ColorLoad::Clear(s.clear),
                );
                CompositeLayer {
                    kind: s.kind,
                    view,
                    placement: s.placement,
                }
            })
            .collect();
        let Some(frame) = lens.host.acquire() else {
            return;
        };
        let target = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        for layer in &layers {
            lens.host.renderer().compose_external_texture(
                &layer.view,
                &target,
                lens.host.format(),
                lw,
                lh,
                layer.placement,
            );
        }
        frame.present();
        // A lens self-capture composes the SAME presented layers (the primary
        // capture discipline, per window).
        if let Some(path) = self.pending_lens_capture.take() {
            if !capture_composed(&lens.host, &layers, lw, lh, &path) {
                tracing::warn!(path = ?path, "lens capture failed");
            }
        }
        if animating {
            lens.window.request_redraw();
        }
    }

    /// Route one lens window's event: pane presses dispatch into the SHARED
    /// retained runners (the same round trips the primary uses); canvas
    /// gestures run with the lens's own camera installed around the pass
    /// (pan, zoom, grab); resize, close.
    pub(super) fn lens_event(&mut self, id: WindowId, event: WindowEvent) {
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                let ordinal = lens.ordinal;
                self.lens_windows.remove(&id);
                if let Some(space) = self.app.lenses.get_mut(ordinal) {
                    *space = None;
                }
                self.app.window_count = 1 + self.lens_windows.len();
                self.app.note(crate::observe::AppEvent::WindowClosed);
                // Persist the departure: a window closed on purpose stays
                // closed across a restart (its slot saves as null).
                self.act(Action::SaveSession);
                return;
            }
            WindowEvent::Resized(size) => {
                lens.width = size.width.max(1);
                lens.height = size.height.max(1);
                lens.host.resize(lens.width, lens.height);
                lens.window.request_redraw();
                return;
            }
            WindowEvent::RedrawRequested => {
                self.render_lens(id);
                return;
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // A press routes by the lens's OWN plan: a pane press
                // dispatches into the shared runner; a tile press routes into
                // the shared SESSION (a link is a durable navigation through
                // the spine); a canvas press falls through to the
                // camera-gesture block below.
                let (x, y) = lens.cursor;
                let (lw, lh, ordinal) = (lens.width, lens.height, lens.ordinal);
                let plan = self.lens_plan(ordinal, lw, lh);
                if let Some(hit) =
                    crate::surface::hit_test(&plan, crate::surface::FocusTarget::Canvas, x, y)
                {
                    match hit.kind {
                        crate::surface::SurfaceKind::Pane(pid) => {
                            // The press anchors pane ops here, exactly as in
                            // the primary — the active pane is GLOBAL (ids are
                            // unique across spaces), so close/divider/summon-
                            // beside now aim at this lens's tree.
                            self.app.active_pane = Some(pid);
                            if let Some(content) = self.lens_pane_content(ordinal, pid) {
                                let dims = plan
                                    .iter()
                                    .find(|s| s.id == hit.id)
                                    .map(|s| {
                                        (
                                            s.rect.w.round().max(1.0) as u32,
                                            s.rect.h.round().max(1.0) as u32,
                                        )
                                    })
                                    .unwrap_or((lw, lh));
                                let actions = self.pane_click_actions(&content, hit.local, dims);
                                for action in actions {
                                    self.act(action);
                                }
                            }
                            self.request_redraw();
                            return;
                        }
                        crate::surface::SurfaceKind::Content(node) => {
                            self.app.focus = crate::surface::FocusTarget::Content(node);
                            if let Some(session) = self.content_sessions.get_mut(&node)
                                && let SessionClick::Navigate(url) =
                                    session.click_at(hit.local.0, hit.local.1)
                            {
                                self.act(Action::OpenAddress(url));
                            }
                            if let Some(lens) = self.lens_windows.get_mut(&id) {
                                lens.window.request_redraw();
                            }
                            return;
                        }
                        // A lens seam drag: capture the band; moves lower
                        // SetSplitRatio at THIS lens's space (same spine as
                        // the primary's seam, different target tree).
                        crate::surface::SurfaceKind::Divider(index) => {
                            if let Some(Some(space)) = self.app.lenses.get(ordinal) {
                                let area = Rect::full(lw, lh);
                                let tiling = crate::pane::place_panes(space, area, None);
                                self.lens_divider_drag = tiling
                                    .dividers
                                    .into_iter()
                                    .find(|d| d.index == index)
                                    .map(|d| (ordinal, d));
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                // Canvas press: handled by the gesture block below.
            }
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x as f32, position.y as f32);
                lens.cursor = (x, y);
                // A held lens seam: each move becomes a ratio through the
                // same component math as the primary's seam, lowered at the
                // LENS's space. Falls through to the camera block otherwise.
                if let Some((ord, drag)) = self.lens_divider_drag.clone() {
                    let split = crate::pane::cambium_split(drag.axis, drag.ratio);
                    let ratio =
                        split.ratio_at(drag.area.w, drag.area.h, x - drag.area.x, y - drag.area.y);
                    self.act(Action::SetSplitRatio {
                        space: crate::action::SpaceRef::Lens(ord),
                        path: drag.path,
                        ratio,
                    });
                    if let Some(lens) = self.lens_windows.get_mut(&id) {
                        lens.window.request_redraw();
                    }
                    return;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                // Like the primary seam: moves rode Redraw; persist once.
                if self.lens_divider_drag.take().is_some() {
                    self.act(Action::SaveSession);
                    return;
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Wheel over a tile scrolls the PAGE (the rung-5 slice-B rule,
                // per window); off-tile falls through to the camera pan below.
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE)
                    }
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                let (x, y) = lens.cursor;
                let (lw, lh, ordinal) = (lens.width, lens.height, lens.ordinal);
                let plan = self.lens_plan(ordinal, lw, lh);
                if let Some(hit) =
                    crate::surface::hit_test(&plan, crate::surface::FocusTarget::Canvas, x, y)
                    && let crate::surface::SurfaceKind::Content(node) = hit.kind
                    && let Some(session) = self.content_sessions.get_mut(&node)
                {
                    if session.scroll_at(hit.local.0, hit.local.1, dx, dy)
                        && let Some(lens) = self.lens_windows.get_mut(&id)
                    {
                        lens.window.request_redraw();
                    }
                    return;
                }
            }
            _ => {}
        }
        // Continuous canvas gestures, with the lens camera installed. The
        // canvas's semantic input methods are the shared vocabulary; only the
        // viewport differs per window (the gesture law holds unchanged).
        let Some(lens) = self.lens_windows.get_mut(&id) else {
            return;
        };
        // The lens camera, with the size it was framed for (see render_lens).
        let saved = self.app.canvas.viewport();
        self.app.canvas.set_viewport(lens.viewport);
        let mut redraw = false;
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                lens.cursor = (position.x as f32, position.y as f32);
                redraw = self.app.canvas.cursor_moved(lens.cursor.0, lens.cursor.1);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_PAN_SCALE, y * WHEEL_PAN_SCALE),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                redraw = self.app.canvas.wheel(dx, dy);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(button) = pointer_button(button) {
                    let (x, y) = lens.cursor;
                    redraw = match state {
                        ElementState::Pressed => {
                            lens.pointer_down = true;
                            self.app.canvas.pointer_down(button, x, y)
                        }
                        ElementState::Released => {
                            lens.pointer_down = false;
                            self.app.canvas.pointer_up(button, x, y)
                        }
                    };
                }
            }
            _ => {}
        }
        // Stash the lens camera back and restore the primary's.
        let lens = self.lens_windows.get_mut(&id).expect("lens still present");
        lens.viewport = self.app.canvas.viewport();
        self.app.canvas.set_viewport(saved);
        if redraw {
            lens.window.request_redraw();
        }
    }
}
