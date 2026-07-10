# Merecat architecture: obviating meerkat

2026-07-10. The founding doc set the role swap (mere = library, merecat = the
reference host); the boundary pass sharpened what mere keeps. This plan is
the other half: what merecat IS, structurally, and the ladder by which
meerkat becomes unnecessary. Companion to
[2026-07-08_merecat_founding.md](./2026-07-08_merecat_founding.md) and mere's
2026-07-09 boundary pass plan (its decisions are assumed here, as amended:
canvas stays mere, platen is the pane home and arrives with the port, the
verso family is serval's `verso-tile`).

## Doctrine

1. **Composition, not migration.** Merecat is assembled from the promoted
   libraries. Meerkat is the donor and the behavioral reference; its modules
   are read for technique and product decisions, never copied wholesale. (The
   same posture the family holds toward graphshell.)
2. **One vocabulary for everything that acts.** Meerkat's largest modules are
   imperative crossroads (`command_drain` 1,196 lines, `shell_eval` 884)
   because commands, input, scripts, scenarios, and UI each grew their own
   mutation paths. Merecat has one: everything lowers to a typed `Action`;
   effects leave through ports; services answer with typed updates. Settings,
   automation (the native-automation plan's WebDriver adapters), scenarios,
   scripting, and remote control all speak Action, so no lane needs a second
   execution model.
3. **Modules first, crates when a boundary proves itself.** The app is one
   crate today. The module map below is the crate map of the future; a module
   is promoted to a crate when a second consumer or a build-condition needs
   the boundary (the same gate the family applies everywhere). No speculative
   workspace sprawl.
4. **Engines arrive through the registry.** The content lane waits for the
   inker adoption (another agent's migration): merecat registers engines and
   composites frames; it never hand-dispatches lanes the way meerkat's
   content actor does. Hand-wiring is the duplication the registry exists to
   prevent.

## The spine

```text
platform event (winit / a11y / automation / scenario)
  -> Action (typed; the one vocabulary)
  -> update(state, action) -> Effect list
  -> ports (fetch actor, physics actor, persistence, engines, comms, ...)
  -> Update (typed service answers, drained on wake)
  -> update(state, update) -> Effect list
  -> projection (canvas Scene now; chrome DOM + pane tiles later)
```

Two rules make this real rather than aspirational:

- `update` never blocks and never touches a platform API. Anything slow or
  platform-shaped is an `Effect` handled by a port.
- Ports are the only owners of actors and stores. State holds data, not
  handles (the shell owns the handles and runs the effects).

## Module map (the future crate map)

| Module | Owns | Today | Crate when |
| --- | --- | --- | --- |
| `action` | `Action`, `Effect`, `Update` enums | this plan's first slice | automation adapters need it without the app |
| `app` | `App` state + the two `update` fns | first slice | never alone; travels with `action` |
| `web` | fetch/enrichment port: page + favicon outcomes into graph stamps; later engine registry + verso-tile flip | exists inline; moves into the module | the engine registry lands (post inker adoption) |
| `session` | persistence port: graph.json now; browser_nodes.json, view intent, settings, multi-session later | exists inline; moves into the module | multi-session lands |
| `shell` | winit + SurfaceHost + input mapping onto canvas semantics + effect runner | today's main.rs | a second host (wasm) appears |
| `ui` | chrome DOM (omnibar, toolbar), pane tiles over platen | absent | it exists at all (needs xilem-serval + platen) |

mere::canvas is hosted, not wrapped: the shell maps raw input onto the
canvas's semantic methods directly (they are already the right vocabulary),
and canvas-affecting Actions call them from `update`'s effect handler. A
merecat-side canvas facade would be indirection with no second consumer.

## The obviation ladder

Meerkat dies when merecat is the daily driver. Each rung names the meerkat
capability, its source of supply, and its gate. Rungs are ordered by
daily-driver value, not by meerkat's module sizes.

1. **Graph canvas + persistence + enrichment** — mere::canvas, fetch,
   session-runtime. LANDED (slices 1-4, 2026-07-09): open address, fetch,
   title/mime/favicon stamps, session restore.
2. **The Action spine** — this plan's first slice: restructure the bin onto
   action/app/web/session/shell modules, behavior-preserving. Gate: none.
3. **Omnibar + navigation chrome** — shell/chrome's host-neutral vocabulary
   (History, NavTarget, suggest) over a xilem-serval chrome document, as a
   second composited layer above the canvas. Gate: none (the vocabulary and
   the render stack both exist). This is the rung that makes merecat usable
   without a terminal argument.
4. **Live content on nodes** — the engine registry via the inker adoption;
   pelt's Engine impls; document scenes for readable pages; scry tiles +
   verso-tile flip for the compat lane. Gate: the mere->serval agent's
   migration (adoption plan done-conditions 1-6).
5. **Panes** — platen (the pane home) + the domain crates (gloss, roster,
   trail, alembic/steward over session-runtime). Gate: rung 3's chrome
   document (panes need a DOM home).
6. **Multi-session + browser-state sidecar** — session-runtime's manifests,
   sessions/<id>/ layout, browser_nodes.json. Gate: rung 4 (per-node browser
   state is worth persisting once nodes hold live content).
7. **Multi-window** — the one-state-N-windows doctrine over the Action spine
   (a window is a projection, so the spine already has the right shape).
   Gate: rung 3.
8. **The long tail** — comms (murm posture), intel (embed/infer glue),
   import/crawl, scripting (rhai + document-host), theming
   (register-theme/tinct), a11y projection. Each is a port + Actions; each
   arrives when wanted, none blocks obviation of the daily-driver set.

**Meerkat's deletion condition** (the founding doc's done-condition 2 made
concrete): rungs 1-6 landed and Mark browses in merecat for a week without
reaching for meerkat. Then meerkat leaves mere's workspace and the mere
facade drops its compatibility re-exports (platen moves here in the same
pass).

## What is deliberately NOT carried

- meerkat's `command_drain` / `shell_eval` execution model (replaced by the
  Action spine; the command REGISTRY vocabulary is worth rereading when rung
  3 adds a command palette).
- The bin-module sprawl (85 modules in main.rs). Merecat's bin stays a shell.
- `PaneContent`/`ContentPane` vocabulary as-is; pane identity gets designed
  at rung 5 against platen's model, not inherited.
- Meerkat's scenario runner as code; the VOCABULARY (one description, two
  runners) returns as Action-driven automation per the native-automation
  plan.

## First slice (this session)

Restructure the existing bin onto the spine, behavior-preserving:
`action.rs` (Action/Effect/Update), `app.rs` (state + update), `web.rs`
(the fetch port + enrichment stamps), `session.rs` (the persistence port),
`shell.rs` (winit + present + input mapping + the effect runner), `main.rs`
(a page of bootstrapping). Done when: the same behaviors hold (open address,
fetch enrichment, favicon, session restore, canvas input), verified by build
plus a headed smoke on the scratch profile, with every mutation path already
flowing through `Action`.

## Progress

- 2026-07-10: Plan written (with Mark: "obviate meerkat, architect merecat";
  the inker/serval migration explicitly belongs to the parallel agent). First
  slice follows in-session.
