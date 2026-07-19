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
//! script <inline lua>        # run a Piccolo control script; its Actions lower
//!                            # through the same spine (the automation runner —
//!                            # needs the `piccolo` feature)
//! click-at <x> <y>          # pointer click at window px (content links, canvas)
//! click-row <substr>       # click the list-pane row whose text contains substr
//! scroll <x> <y> <dy>       # wheel at window px (page scroll / canvas pan)
//! divider <ratio>           # set the active pane's split ratio (0.0-1.0)
//! assert pane <tag>         # a pane with that PaneContent tag is in the tree
//! assert maximized | not-maximized
//! drop-file <x> <y> <path>  # drop a file at window px (image on node = sprite;
//!                           # else it becomes a file:// node)
//! drag-tab <a> onto <b>     # drag workbench tab <a> onto tab <b>'s cell (stack)
//! drag-tab <a> onto <b> @ <edge>  # ...releasing on the cell's left|right|top|bottom
//!                           # edge band (split beside instead of stack)
//! drag-tab <a> out          # ...releasing OUTSIDE the workbench (on the canvas):
//!                           # the tile tears out into a lens window (branch arm)
//! assert row <substr>       # a Trail/Roster/Inspector row's text contains substr
//! assert wb-cells ==|>=|<= <n>  # the workbench has n cells
//! assert wb-cell <substr>   # a workbench cell's tab string contains substr
//! assert wb-fraction ==|>=|<= <f>  # the workbench root split's FIRST fraction
//! assert a11y <substr>      # an a11y-projection line ("role: label") contains substr
//! assert lens-pane <substr> # a lens window's "ordinal:tag" pane contains substr
//! assert lens-surface <kind> # the FIRST lens window's live plan composites that kind
//! assert no-pane <tag>      # NO pane with that tag is in the PRIMARY tree
//! assert no-lens-pane <substr> # NO lens "ordinal:tag" pane contains substr
//! assert no-surface <kind>  # NO surface of that kind in the PRIMARY plan
//! assert active-ratio ==|>=|<= <f> # the ACTIVE pane's parent-split ratio (any space)
//! assert sessions ==|>=|<= <n>  # the manifest set holds n sessions
//! assert session <substr>   # the live session's label contains substr
//! assert nodes ==|>=|<= <n> # the graph holds n nodes
//! capture-lens <name>       # self-capture the first lens window's frame
//! assert omnibar open|closed
//! assert omnibar-text <str>  # the omnibar text is exactly <str>
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


/// One parsed scenario step. `MERECAT_SCENARIO` runs on the shared
/// `genet_probe::Scenario` loop; the generic verbs (act/settle/capture/log,
/// assert text/event/snap) it owns, and these app-specific steps reach the Shell
/// via `Driveable::app_step`, parsed here and run by `Shell::run_scenario_step`.
#[derive(Debug)]
pub enum Step {
    Open(String),
    Omnibar { command: bool },
    Type(String),
    Insert(String),
    Key(EditKey),
    Act(String),
    /// A Piccolo control script whose emitted Actions lower through the spine
    /// (the "one description, two runners" automation lane). Piccolo-gated.
    Script(String),
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
    /// Click the Roster's tab by label; the shell asks the strip where it is.
    ClickTab(String),
    /// Click the Gloss minimap's node by label/url substring.
    ClickNode(String),
    /// Press at the first point, move through it, release at the second.
    Drag((f32, f32), (f32, f32)),
    /// Drag the workbench tab labelled like the first substring onto the cell
    /// of the tab labelled like the second (the stack gesture, by name). The
    /// optional edge releases on that band of the cell body (split beside).
    DragTab(String, String, Option<String>),
    /// Drag workbench tab <a> and release OUTSIDE the workbench pane (on the
    /// canvas) — the tile tear-out gesture (the trichotomy's branch arm).
    DragTabOut(String),
    /// Drop the file at window `(x, y)` — the same handler winit's
    /// `DroppedFile` drives.
    DropFile(f32, f32, String),
    /// The workbench has this many cells.
    AssertWbCells(CmpOp, usize),
    /// A workbench cell's tab string contains this substring.
    AssertWbCell(String),
    /// The workbench root split's FIRST fraction compares as given.
    AssertWbFraction(CmpOp, f32),
    /// An a11y-projection line contains this substring.
    AssertA11y(String),
    /// The window count (primary + lenses) compares as given.
    AssertWindows(CmpOp, usize),
    /// A lens window holds a pane whose "ordinal:tag" contains the substring.
    AssertLensPane(String),
    /// The FIRST lens window's live surface plan composites a surface of the
    /// named kind (the tiles-follow-the-pane receipt).
    AssertLensSurface(String),
    /// NO pane with the given tag is in the PRIMARY tree (the tear-out's
    /// departure half).
    AssertNoPane(String),
    /// NO surface of the named kind is in the PRIMARY plan (the one-session-
    /// one-surface rule's cross-window half).
    AssertNoSurface(String),
    /// NO lens pane's "ordinal:tag" contains the substring (the lens close
    /// op's departure half).
    AssertNoLensPane(String),
    /// The ACTIVE pane's parent-split ratio compares as given, in whichever
    /// space holds the pane — the divider op's readback, primary or lens.
    AssertActiveRatio(CmpOp, f32),
    /// The manifest set's session count compares as given.
    AssertSessions(CmpOp, usize),
    /// The live session's label contains this substring.
    AssertSession(String),
    /// The graph's node count compares as given (a switch shows a different
    /// graph; this is the cheap witness).
    AssertNodes(CmpOp, usize),
    /// Self-capture the first live lens window's composed frame.
    CaptureLens(String),
    /// The root split's ratio compares as given.
    AssertRatio(CmpOp, f32),
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
    /// A list/detail pane row's text contains this substring.
    AssertRow(String),
    AssertTab(String),
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
pub enum EditKey {
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
pub enum CmpOp {
    Eq,
    Ge,
    Le,
}

/// Parse "`<x> <y>`" into a pair of f32 window coordinates.
fn parse_xy(s: &str) -> Option<(f32, f32)> {
    let mut it = s.split_whitespace();
    let x = it.next()?.parse().ok()?;
    let y = it.next()?.parse().ok()?;
    Some((x, y))
}

pub fn parse(body: &str) -> Result<Vec<Step>, String> {
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
            "script" if !rest.is_empty() => Step::Script(rest.to_string()),
            "settle" => Step::Settle(if rest.is_empty() {
                20
            } else {
                rest.parse().map_err(|_| format!("line {}: bad settle count '{rest}'", i + 1))?
            }),
            "click-row" if !rest.is_empty() => Step::ClickRow(rest.to_string()),
            "drop-file" => {
                let mut it = rest.splitn(3, char::is_whitespace);
                let x = it.next().and_then(|t| t.parse().ok());
                let y = it.next().and_then(|t| t.parse().ok());
                let path = it.next().map(str::trim).filter(|p| !p.is_empty());
                match (x, y, path) {
                    (Some(x), Some(y), Some(path)) => Step::DropFile(x, y, path.to_string()),
                    _ => return err("drop-file wants '<x> <y> <path>'"),
                }
            }
            "drag-tab" => {
                // `drag-tab <a> out` releases OUTSIDE the workbench pane (on
                // the canvas): the tear-out trichotomy's branch arm.
                if let Some(from) = rest
                    .strip_suffix(" out")
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                {
                    Step::DragTabOut(from.to_string())
                } else {
                    let (from, onto) = rest
                        .split_once(" onto ")
                        .map(|(a, b)| (a.trim(), b.trim()))
                        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
                        .ok_or_else(|| {
                            format!("line {}: drag-tab wants '<a> onto <b>': '{line}'", i + 1)
                        })?;
                    let (onto, edge) = match onto.split_once(" @ ") {
                        Some((b, e)) => {
                            let e = e.trim();
                            if !matches!(e, "left" | "right" | "top" | "bottom") {
                                return err("drag-tab edge wants left|right|top|bottom");
                            }
                            (b.trim(), Some(e.to_string()))
                        }
                        None => (onto, None),
                    };
                    Step::DragTab(from.to_string(), onto.to_string(), edge)
                }
            }
            "click-tab" if !rest.is_empty() => Step::ClickTab(rest.to_string()),
            "click-node" if !rest.is_empty() => Step::ClickNode(rest.to_string()),
            "drag" => {
                let nums: Vec<f32> = rest
                    .split_whitespace()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                match nums[..] {
                    [x1, y1, x2, y2] => Step::Drag((x1, y1), (x2, y2)),
                    _ => return err("drag wants 'x1 y1 x2 y2'"),
                }
            }
            "click-at" => {
                let (x, y) = parse_xy(rest).ok_or_else(|| {
                    format!("line {}: click-at wants '<x> <y>': '{line}'", i + 1)
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
            "capture-lens" if !rest.is_empty() => Step::CaptureLens(rest.to_string()),
            "assert" => {
                let (what, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                let arg = arg.trim();
                match what {
                    "omnibar" => match arg {
                        "open" => Step::AssertOmnibar(true),
                        "closed" => Step::AssertOmnibar(false),
                        _ => return err("assert omnibar wants open|closed"),
                    },
                    "omnibar-text" => Step::AssertText(arg.to_string()),
                    "event" if !arg.is_empty() => Step::AssertEvent(arg.to_string()),
                    "focused" if !arg.is_empty() => Step::AssertFocused(arg.to_string()),
                    "surface" if !arg.is_empty() => Step::AssertSurface(arg.to_string()),
                    "focus" if !arg.is_empty() => Step::AssertFocus(arg.to_string()),
                    "pane" if !arg.is_empty() => Step::AssertPane(arg.to_string()),
                    "maximized" => Step::AssertMaximized(true),
                    "not-maximized" => Step::AssertMaximized(false),
                    "row" if !arg.is_empty() => Step::AssertRow(arg.to_string()),
                    "tab" if !arg.is_empty() => Step::AssertTab(arg.to_string()),
                    "a11y" if !arg.is_empty() => Step::AssertA11y(arg.to_string()),
                    "lens-pane" if !arg.is_empty() => Step::AssertLensPane(arg.to_string()),
                    "lens-surface" if !arg.is_empty() => {
                        Step::AssertLensSurface(arg.to_string())
                    }
                    "no-pane" if !arg.is_empty() => Step::AssertNoPane(arg.to_string()),
                    "no-lens-pane" if !arg.is_empty() => {
                        Step::AssertNoLensPane(arg.to_string())
                    }
                    "no-surface" if !arg.is_empty() => Step::AssertNoSurface(arg.to_string()),
                    "windows" | "sessions" | "nodes" => {
                        let (op, n) = arg.split_once(char::is_whitespace).ok_or_else(|| {
                            format!("line {}: assert {what} wants '<op> <n>'", i + 1)
                        })?;
                        let op = match op {
                            "==" => CmpOp::Eq,
                            ">=" => CmpOp::Ge,
                            "<=" => CmpOp::Le,
                            _ => return err("assert count op wants ==|>=|<="),
                        };
                        let n = n
                            .trim()
                            .parse()
                            .map_err(|_| format!("line {}: bad {what} count", i + 1))?;
                        match what {
                            "sessions" => Step::AssertSessions(op, n),
                            "nodes" => Step::AssertNodes(op, n),
                            _ => Step::AssertWindows(op, n),
                        }
                    }
                    "session" if !arg.is_empty() => Step::AssertSession(arg.to_string()),
                    "wb-cell" if !arg.is_empty() => Step::AssertWbCell(arg.to_string()),
                    "wb-cells" => {
                        let (op, n) = arg
                            .split_once(char::is_whitespace)
                            .ok_or_else(|| format!("line {}: assert wb-cells wants '<op> <n>'", i + 1))?;
                        let op = match op {
                            "==" => CmpOp::Eq,
                            ">=" => CmpOp::Ge,
                            "<=" => CmpOp::Le,
                            _ => return err("assert wb-cells op wants ==|>=|<="),
                        };
                        let n = n
                            .trim()
                            .parse()
                            .map_err(|_| format!("line {}: bad wb-cells count", i + 1))?;
                        Step::AssertWbCells(op, n)
                    }
                    "wb-fraction" => {
                        let (op, f) = arg
                            .split_once(char::is_whitespace)
                            .ok_or_else(|| format!("line {}: assert wb-fraction wants '<op> <f>'", i + 1))?;
                        let op = match op {
                            "==" => CmpOp::Eq,
                            ">=" => CmpOp::Ge,
                            "<=" => CmpOp::Le,
                            _ => return err("assert wb-fraction op wants ==|>=|<="),
                        };
                        let f = f
                            .trim()
                            .parse()
                            .map_err(|_| format!("line {}: bad wb-fraction", i + 1))?;
                        Step::AssertWbFraction(op, f)
                    }
                    "ratio" | "active-ratio" => {
                        let (op, n) = arg
                            .split_once(char::is_whitespace)
                            .ok_or_else(|| format!("line {}: assert ratio wants '<op> <r>'", i + 1))?;
                        let op = match op {
                            "==" => CmpOp::Eq,
                            ">=" => CmpOp::Ge,
                            "<=" => CmpOp::Le,
                            _ => return err("assert ratio op wants ==|>=|<="),
                        };
                        let n = n
                            .trim()
                            .parse()
                            .map_err(|_| format!("line {}: bad ratio", i + 1))?;
                        if what == "active-ratio" {
                            Step::AssertActiveRatio(op, n)
                        } else {
                            Step::AssertRatio(op, n)
                        }
                    }
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

    #[test]
    fn parse_errors_name_their_line() {
        let err = parse("open mere://x
frobnicate
").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }
}
