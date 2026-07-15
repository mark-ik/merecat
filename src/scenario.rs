//! The self-drive scenario lane: the app drives ITSELF from a scenario file,
//! so headed receipts need no OS synthetic input — no focus race, no key
//! theft when a human is using the machine (the SendKeys lane's unfixable
//! failure mode). This is the vocabulary's first automation consumer (the
//! architecture plan's recorded trigger): every step lowers to an ordinary
//! [`Action`] through the same `update` spine as a keypress, so the runner
//! needs no second execution model.
//!
//! Activation: set `MERECAT_SCENARIO` to a scenario file path before launch;
//! captures land in `MERECAT_CAPTURE_DIR` (default: beside the scenario).
//! The run ends by writing `scenario.done` there — first line `RESULT ok` or
//! `RESULT fail`, then the log lines — and exiting WITHOUT saving the
//! session, so a scenario never mutates the profile it ran against and
//! reruns stay deterministic.
//!
//! Grammar (one verb per line, `#` comments):
//!
//! ```text
//! settle [frames]           # pump N frames (default 20)
//! capture <name>            # self-capture the composed frame -> <name>.png
//! open <url>                # Action::OpenAddress (use mere:// for offline)
//! omnibar find|actions      # Action::OmnibarOpen
//! type <text>               # Action::OmnibarChar per char
//! insert <text>             # Action::OmnibarInsert (the IME-commit path)
//! key enter|escape|backspace|delete|up|down|left|right|home|end
//! act <palette label>       # commit a palette_actions() entry by label
//! click <x> <y>             # pointer click at window px (content links, canvas)
//! click-row <substr>       # click the list-pane row whose text contains substr
//! scroll <x> <y> <dy>       # wheel at window px (page scroll / canvas pan)
//! divider <ratio>           # set the active pane's split ratio (0.0-1.0)
//! assert pane <tag>         # a pane with that PaneContent tag is in the tree
//! assert maximized | not-maximized
//! assert row <substr>       # a Trail pane row's text contains substr
//! assert omnibar open|closed
//! assert text <str>         # the omnibar text is exactly <str>
//! assert focused <substr>   # focused node's url/caption contains substr
//! assert surface <kind>     # a surface of that kind (canvas|content|chrome) is composited
//! assert focus <kind>       # semantic input goes to that surface kind
//! assert suggestions ==|>=|<= <n>
//! assert visible            # at least one node inside the viewport
//! assert event <substr>     # a semantic event matching <substr> was emitted
//! log <text>
//! ```
//!
//! Asserts read the observation surface ([`crate::observe`]) — the same
//! snapshot/event pair the a11y and automation lanes consume — so a green
//! scenario certifies the surface those lanes will stand on.

use std::path::{Path, PathBuf};

use crate::action::{Action, palette_actions};
use crate::app::App;

/// A parsed scenario plus its run state. The shell pumps [`Scenario::tick`]
/// once per rendered frame; one step is consumed per frame, so every step
/// observes a fully projected app state.
pub struct Scenario {
    steps: Vec<Step>,
    idx: usize,
    settle: u32,
    log: Vec<String>,
    failed: bool,
    out_dir: PathBuf,
    /// The app's semantic event stream, drained by the shell each frame
    /// (described strings; `assert event` matches substrings against them).
    events: Vec<String>,
}

#[derive(Debug)]
enum Step {
    Open(String),
    Omnibar { command: bool },
    Type(String),
    Insert(String),
    Key(EditKey),
    Act(String),
    /// A pointer press+release at window pixel coordinates (rung 5 slice B).
    /// Drives the same surface-routed path winit does; a click on content
    /// resolves links, a click on the canvas is a canvas gesture.
    Click(f32, f32),
    /// A wheel event at window pixel coordinates: content scrolls, canvas pans.
    Scroll(f32, f32, f32, f32),
    /// Click the list-pane (Trail/Roster) row whose text contains this substring.
    /// The shell resolves the row's window position (it owns the pane rects and
    /// rows), so the receipt names a row by text, not pixels.
    ClickRow(String),
    Settle(u32),
    Capture(String),
    AssertOmnibar(bool),
    AssertText(String),
    AssertFocused(String),
    /// A named surface kind ("canvas" / "content" / "chrome" / "pane") is in the plan.
    AssertSurface(String),
    /// The focus target is a named surface kind.
    AssertFocus(String),
    /// A pane with the given `PaneContent` tag is in the frisket tree.
    AssertPane(String),
    /// Whether a pane is maximized.
    AssertMaximized(bool),
    /// A Trail pane row's text contains this substring.
    AssertRow(String),
    /// Set the active pane's divider ratio (drag the seam).
    Divider(f32),
    AssertSuggestions(CmpOp, usize),
    AssertVisible,
    /// The focused node's content lifecycle is Live (the phase-4 receipt's
    /// app-truth half; the capture is the pixel half).
    AssertContentLive,
    /// A semantic event whose description contains the substring was emitted
    /// at some point this run.
    AssertEvent(String),
    Log(String),
}

#[derive(Debug)]
enum EditKey {
    Enter,
    Escape,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

#[derive(Debug)]
enum CmpOp {
    Eq,
    Ge,
    Le,
}

/// What the shell should do this frame.
pub enum Tick {
    /// Lower these actions through the spine, then keep pumping frames.
    Act(Vec<Action>),
    /// A pointer click at window coordinates, routed through the shell's shared
    /// surface path (the shell owns the sessions the scenario cannot see).
    Click { x: f32, y: f32 },
    /// A wheel event at window coordinates, routed the same way.
    Scroll { x: f32, y: f32, dx: f32, dy: f32 },
    /// Click the list-pane row containing this substring; the shell resolves it.
    ClickRow { substr: String },
    /// Still settling; pump another frame.
    Wait,
    /// Compose and read back the current frame to this path.
    Capture(PathBuf),
    /// Every step is consumed: write the sentinel and exit the event loop.
    Done,
}

impl Scenario {
    /// The env-var activation seam. A parse error still yields a scenario —
    /// one that is already failed with the error in its log — so the driver
    /// waiting on `scenario.done` learns WHY instead of timing out.
    pub fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os("MERECAT_SCENARIO")?);
        let out_dir = std::env::var_os("MERECAT_CAPTURE_DIR")
            .map(PathBuf::from)
            .or_else(|| path.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let _ = std::fs::create_dir_all(&out_dir);
        Some(match std::fs::read_to_string(&path) {
            Ok(body) => match parse(&body) {
                Ok(steps) => Self {
                    steps,
                    idx: 0,
                    settle: 0,
                    log: Vec::new(),
                    failed: false,
                    out_dir,
                    events: Vec::new(),
                },
                Err(err) => Self::stillborn(out_dir, format!("parse error: {err}")),
            },
            Err(err) => Self::stillborn(out_dir, format!("unreadable scenario {path:?}: {err}")),
        })
    }

    fn stillborn(out_dir: PathBuf, why: String) -> Self {
        Self {
            steps: Vec::new(),
            idx: 0,
            settle: 0,
            log: vec![format!("FAIL: {why}")],
            failed: true,
            out_dir,
            events: Vec::new(),
        }
    }

    /// Fold the frame's drained semantic events into the run (the shell
    /// calls this each pump; `assert event` and failure diagnostics read it).
    pub fn note_events(&mut self, events: &[crate::observe::AppEvent]) {
        self.events
            .extend(events.iter().map(crate::observe::AppEvent::describe));
    }

    /// Consume at most one step against the current app state.
    pub fn tick(&mut self, app: &App) -> Tick {
        if self.settle > 0 {
            self.settle -= 1;
            return Tick::Wait;
        }
        let Some(step) = self.steps.get(self.idx) else {
            return Tick::Done;
        };
        self.idx += 1;
        match step {
            Step::Open(url) => Tick::Act(vec![Action::OpenAddress(url.clone())]),
            Step::Omnibar { command } => Tick::Act(vec![Action::OmnibarOpen { command: *command }]),
            Step::Type(text) => Tick::Act(text.chars().map(Action::OmnibarChar).collect()),
            Step::Insert(text) => Tick::Act(vec![Action::OmnibarInsert(text.clone())]),
            Step::Key(key) => {
                use crate::action::CaretMove;
                Tick::Act(vec![match key {
                    EditKey::Enter => Action::OmnibarCommit,
                    EditKey::Escape => Action::OmnibarClose,
                    EditKey::Backspace => Action::OmnibarBackspace,
                    EditKey::Delete => Action::OmnibarDelete,
                    EditKey::Up => Action::OmnibarMove(-1),
                    EditKey::Down => Action::OmnibarMove(1),
                    EditKey::Left => Action::OmnibarCaret(CaretMove::Left),
                    EditKey::Right => Action::OmnibarCaret(CaretMove::Right),
                    EditKey::Home => Action::OmnibarCaret(CaretMove::Home),
                    EditKey::End => Action::OmnibarCaret(CaretMove::End),
                }])
            }
            Step::Act(label) => {
                let wanted = label.to_lowercase();
                match palette_actions()
                    .into_iter()
                    .find(|(l, _)| l.to_lowercase() == wanted)
                {
                    Some((_, action)) => Tick::Act(vec![action]),
                    None => {
                        self.fail(format!("act: no palette action labelled '{label}'"));
                        Tick::Wait
                    }
                }
            }
            Step::Settle(frames) => {
                self.settle = *frames;
                Tick::Wait
            }
            Step::Click(x, y) => Tick::Click { x: *x, y: *y },
            Step::ClickRow(substr) => Tick::ClickRow {
                substr: substr.clone(),
            },
            Step::Scroll(x, y, dx, dy) => Tick::Scroll {
                x: *x,
                y: *y,
                dx: *dx,
                dy: *dy,
            },
            Step::Capture(name) => Tick::Capture(self.out_dir.join(format!("{name}.png"))),
            // Asserts read the observation surface — the same snapshot the
            // a11y/automation lanes consume — never app fields directly.
            Step::AssertOmnibar(open) => {
                let snap = crate::observe::snapshot(app);
                if snap.omnibar.open != *open {
                    let state = if *open { "open" } else { "closed" };
                    self.fail(format!("assert omnibar {state}: it is not"));
                }
                Tick::Wait
            }
            Step::AssertText(want) => {
                let snap = crate::observe::snapshot(app);
                if snap.omnibar.text != *want {
                    self.fail(format!(
                        "assert text '{want}': the omnibar holds '{}'",
                        snap.omnibar.text
                    ));
                }
                Tick::Wait
            }
            Step::AssertFocused(substr) => {
                let needle = substr.to_lowercase();
                let snap = crate::observe::snapshot(app);
                let hay = snap
                    .focused
                    .map(|f| format!("{} {}", f.url, f.caption).to_lowercase())
                    .unwrap_or_default();
                if !hay.contains(&needle) {
                    self.fail(format!("assert focused '{substr}': focused is '{hay}'"));
                }
                Tick::Wait
            }
            Step::AssertSurface(kind) => {
                let snap = crate::observe::snapshot(app);
                if !snap.surfaces.iter().any(|s| s == kind) {
                    self.fail(format!(
                        "assert surface '{kind}': the plan is {:?}",
                        snap.surfaces
                    ));
                }
                Tick::Wait
            }
            Step::AssertFocus(kind) => {
                let snap = crate::observe::snapshot(app);
                if snap.focus != *kind {
                    self.fail(format!(
                        "assert focus '{kind}': focus is '{}'",
                        snap.focus
                    ));
                }
                Tick::Wait
            }
            Step::AssertPane(tag) => {
                let snap = crate::observe::snapshot(app);
                if !snap.panes.iter().any(|p| p == tag) {
                    self.fail(format!(
                        "assert pane '{tag}': the tree holds {:?}",
                        snap.panes
                    ));
                }
                Tick::Wait
            }
            Step::AssertMaximized(want) => {
                let snap = crate::observe::snapshot(app);
                if snap.maximized != *want {
                    let state = if *want { "maximized" } else { "not maximized" };
                    self.fail(format!("assert {state}: it is not"));
                }
                Tick::Wait
            }
            Step::AssertRow(substr) => {
                let snap = crate::observe::snapshot(app);
                let hit = snap
                    .trail_rows
                    .iter()
                    .chain(snap.roster_rows.iter())
                    .any(|r| r.contains(substr));
                if !hit {
                    self.fail(format!(
                        "assert row '{substr}': trail {:?} roster {:?}",
                        snap.trail_rows, snap.roster_rows
                    ));
                }
                Tick::Wait
            }
            Step::Divider(ratio) => {
                Tick::Act(vec![Action::SetActivePaneDivider(*ratio)])
            }
            Step::AssertSuggestions(op, n) => {
                let snap = crate::observe::snapshot(app);
                let len = snap.omnibar.suggestions.len();
                let ok = match op {
                    CmpOp::Eq => len == *n,
                    CmpOp::Ge => len >= *n,
                    CmpOp::Le => len <= *n,
                };
                if !ok {
                    self.fail(format!(
                        "assert suggestions: have {len} ({:?}), wanted {n}",
                        snap.omnibar.suggestions
                    ));
                }
                Tick::Wait
            }
            Step::AssertVisible => {
                if !crate::observe::snapshot(app).graph_visible {
                    self.fail("assert visible: every node is off-screen".to_string());
                }
                Tick::Wait
            }
            Step::AssertContentLive => {
                let snap = crate::observe::snapshot(app);
                let focused = snap.focused.as_ref().map(|f| f.member);
                let state = focused
                    .and_then(|id| snap.content.iter().find(|(n, _)| *n == id))
                    .map(|(_, s)| s.clone());
                if state.as_deref() != Some("live") {
                    self.fail(format!(
                        "assert content-live: focused node is {}",
                        state.unwrap_or_else(|| "without content state".to_string())
                    ));
                }
                Tick::Wait
            }
            Step::AssertEvent(substr) => {
                if !self.events.iter().any(|e| e.contains(substr.as_str())) {
                    self.fail(format!(
                        "assert event '{substr}': not in the stream (last: {:?})",
                        self.events.iter().rev().take(6).collect::<Vec<_>>()
                    ));
                }
                Tick::Wait
            }
            Step::Log(text) => {
                self.log.push(text.clone());
                Tick::Wait
            }
        }
    }

    /// Record a capture's outcome (the shell owns the GPU work).
    pub fn note_capture(&mut self, path: &Path, ok: bool) {
        if ok {
            self.log.push(format!("captured {}", path.display()));
        } else {
            self.fail(format!("capture failed: {}", path.display()));
        }
    }

    fn fail(&mut self, why: String) {
        self.failed = true;
        self.log.push(format!("FAIL: {why}"));
    }

    /// Write the `scenario.done` sentinel the driver waits on. First line is
    /// `RESULT ok`/`RESULT fail`, then the log lines (the same shape
    /// meerkat's `Run-Scenario` parses).
    pub fn finish(&self) {
        let result = if self.failed { "fail" } else { "ok" };
        let mut body = format!("RESULT {result}\n");
        for line in &self.log {
            body.push_str(line);
            body.push('\n');
        }
        // On failure, the event tail is the diagnosis: what actually
        // happened, in semantic terms, right before things went wrong.
        if self.failed {
            for event in self.events.iter().rev().take(12).rev() {
                body.push_str("event: ");
                body.push_str(event);
                body.push('\n');
            }
        }
        let _ = std::fs::write(self.out_dir.join("scenario.done"), body);
    }
}

/// Parse "`<x> <y>`" into a pair of f32 window coordinates.
fn parse_xy(s: &str) -> Option<(f32, f32)> {
    let mut it = s.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    Some((x, y))
}

fn parse(body: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (i, raw) in body.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let err = |msg: &str| Err(format!("line {}: {msg}: '{line}'", i + 1));
        steps.push(match verb {
            "open" if !rest.is_empty() => Step::Open(rest.to_string()),
            "omnibar" => match rest {
                "find" => Step::Omnibar { command: false },
                "actions" => Step::Omnibar { command: true },
                _ => return err("omnibar wants find|actions"),
            },
            "type" if !rest.is_empty() => Step::Type(rest.to_string()),
            "insert" if !rest.is_empty() => Step::Insert(rest.to_string()),
            "key" => match rest {
                "enter" => Step::Key(EditKey::Enter),
                "escape" => Step::Key(EditKey::Escape),
                "backspace" => Step::Key(EditKey::Backspace),
                "delete" => Step::Key(EditKey::Delete),
                "up" => Step::Key(EditKey::Up),
                "down" => Step::Key(EditKey::Down),
                "left" => Step::Key(EditKey::Left),
                "right" => Step::Key(EditKey::Right),
                "home" => Step::Key(EditKey::Home),
                "end" => Step::Key(EditKey::End),
                _ => {
                    return err(
                        "key wants enter|escape|backspace|delete|up|down|left|right|home|end",
                    );
                }
            },
            "act" if !rest.is_empty() => Step::Act(rest.to_string()),
            "settle" => Step::Settle(if rest.is_empty() {
                20
            } else {
                rest.parse().map_err(|_| format!("line {}: bad settle count '{rest}'", i + 1))?
            }),
            "click-row" if !rest.is_empty() => Step::ClickRow(rest.to_string()),
            "click" => {
                let (x, y) = parse_xy(rest).ok_or_else(|| {
                    format!("line {}: click wants '<x> <y>': '{line}'", i + 1)
                })?;
                Step::Click(x, y)
            }
            "scroll" => {
                let mut it = rest.split_whitespace();
                let vals: Vec<f32> = it.by_ref().filter_map(|t| t.parse().ok()).collect();
                match vals.as_slice() {
                    // `scroll <x> <y> <dy>` (horizontal delta defaults to 0).
                    [x, y, dy] => Step::Scroll(*x, *y, 0.0, *dy),
                    [x, y, dx, dy] => Step::Scroll(*x, *y, *dx, *dy),
                    _ => return err("scroll wants '<x> <y> <dy>' or '<x> <y> <dx> <dy>'"),
                }
            }
            "divider" => {
                let ratio = rest
                    .parse()
                    .map_err(|_| format!("line {}: divider wants a ratio: '{line}'", i + 1))?;
                Step::Divider(ratio)
            }
            "capture" if !rest.is_empty() => Step::Capture(rest.to_string()),
            "assert" => {
                let (what, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let arg = arg.trim();
                match what {
                    "omnibar" => match arg {
                        "open" => Step::AssertOmnibar(true),
                        "closed" => Step::AssertOmnibar(false),
                        _ => return err("assert omnibar wants open|closed"),
                    },
                    "text" => Step::AssertText(arg.to_string()),
                    "event" if !arg.is_empty() => Step::AssertEvent(arg.to_string()),
                    "focused" if !arg.is_empty() => Step::AssertFocused(arg.to_string()),
                    "surface" if !arg.is_empty() => Step::AssertSurface(arg.to_string()),
                    "focus" if !arg.is_empty() => Step::AssertFocus(arg.to_string()),
                    "pane" if !arg.is_empty() => Step::AssertPane(arg.to_string()),
                    "maximized" => Step::AssertMaximized(true),
                    "not-maximized" => Step::AssertMaximized(false),
                    "row" if !arg.is_empty() => Step::AssertRow(arg.to_string()),
                    "suggestions" => {
                        let (op, n) = arg
                            .split_once(char::is_whitespace)
                            .ok_or_else(|| format!("line {}: assert suggestions wants '<op> <n>'", i + 1))?;
                        let op = match op {
                            "==" => CmpOp::Eq,
                            ">=" => CmpOp::Ge,
                            "<=" => CmpOp::Le,
                            _ => return err("assert suggestions op wants ==|>=|<="),
                        };
                        let n = n
                            .trim()
                            .parse()
                            .map_err(|_| format!("line {}: bad suggestion count", i + 1))?;
                        Step::AssertSuggestions(op, n)
                    }
                    "visible" => Step::AssertVisible,
                    "content-live" => Step::AssertContentLive,
                    _ => return err("unknown assert"),
                }
            }
            "log" => Step::Log(rest.to_string()),
            _ => return err("unknown verb"),
        });
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenario(body: &str) -> Scenario {
        Scenario {
            steps: parse(body).expect("scenario parses"),
            idx: 0,
            settle: 0,
            log: Vec::new(),
            failed: false,
            out_dir: std::env::temp_dir(),
            events: Vec::new(),
        }
    }

    /// Drive a scenario against a real App the way the shell does, minus the
    /// GPU: Act ticks lower through `update`, captures are acknowledged.
    fn run(sc: &mut Scenario, app: &mut App) {
        loop {
            let tick = sc.tick(app);
            sc.note_events(&app.take_events());
            match tick {
                Tick::Act(actions) => {
                    for a in actions {
                        app.update(a);
                    }
                    sc.note_events(&app.take_events());
                }
                Tick::Wait => {}
                // Pointer ticks route through the shell's surface plan + live
                // sessions, which this App-only driver has not got; the headed
                // scenario exercises them. No-op here.
                Tick::Click { .. } | Tick::Scroll { .. } | Tick::ClickRow { .. } => {}
                Tick::Capture(path) => sc.note_capture(&path, true),
                Tick::Done => break,
            }
        }
    }

    #[test]
    fn a_full_scenario_drives_the_spine_and_passes() {
        let mut app = App::test_stub();
        let mut sc = scenario(
            "# rung 3 smoke\n\
             open mere://alpha\n\
             open mere://beta\n\
             omnibar find\n\
             type alp\n\
             assert omnibar open\n\
             assert suggestions >= 1\n\
             key enter\n\
             assert focused alpha\n\
             assert omnibar closed\n\
             omnibar actions\n\
             type re\n\
             assert suggestions >= 1\n\
             key escape\n\
             act Toggle isometric view\n\
             assert event address-opened mere://alpha\n\
             assert event omnibar-committed\n\
             assert event omnibar-closed\n\
             log done\n",
        );
        run(&mut sc, &mut app);
        assert!(!sc.failed, "log: {:?}", sc.log);
        assert!(app.canvas.is_isometric(), "the act step ran the registry action");
    }

    /// Caret keys and the IME-commit insert edit at the cursor, driven
    /// through the scenario vocabulary like any other intent.
    #[test]
    fn caret_keys_edit_at_the_cursor() {
        let mut app = App::test_stub();
        let mut sc = scenario(
            "omnibar find\n\
             type abd\n\
             key left\n\
             insert c\n\
             assert text abcd\n\
             key home\n\
             key delete\n\
             assert text bcd\n\
             key end\n\
             key backspace\n\
             assert text bc\n",
        );
        run(&mut sc, &mut app);
        assert!(!sc.failed, "log: {:?}", sc.log);
    }

    #[test]
    fn a_failed_assert_marks_the_run_and_names_itself() {
        let mut app = App::test_stub();
        let mut sc = scenario("assert focused nothing-is-focused-yet\n");
        run(&mut sc, &mut app);
        assert!(sc.failed);
        assert!(sc.log[0].contains("assert focused"), "log: {:?}", sc.log);
    }

    #[test]
    fn parse_errors_name_their_line() {
        let err = parse("open mere://x\nfrobnicate\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }
}
