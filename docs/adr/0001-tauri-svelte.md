# ADR-0001: Tauri 2 with a Rust core and a Svelte 5 frontend

**Status:** Accepted
**Date:** 2026-10-13 (the decision was made when the project was scaffolded; this records it)
**Deciders:** Adriaan Cloet

## Context

SimpleCalendar is a lightweight, local-first desktop calendar. The forces behind the stack
choice:

- **Low memory comes first.** A calendar is open all day, so its idle footprint matters more
  than for an app that is opened briefly.
- **Two desktop platforms now, mobile later.** Ubuntu and Windows are the targets. Development
  happens on Ubuntu; Windows is built in CI and tested by hand.
- **The hard parts are not UI.** Recurrence rules, time zones, sync and conflict handling need
  to be correct and well tested, and should not be tied to a UI toolkit.
- **The UI should look polished:** minimal, Apple-like, generous whitespace, subtle animation,
  light and dark mode.
- **Credentials belong in the OS keychain,** which requires native access.
- **Learning Rust is an explicit goal** of the project.
- One developer, no deadline.

The options below are the ones that were actually weighed. The reasoning was written down
afterwards, from the project principles in `CLAUDE.md`. No comparison between the options was
benchmarked; the assessments are qualitative. The memory use of the chosen stack was measured
afterwards and is recorded under "To revisit".

## Decision

Build the app with **Tauri 2**: a Rust process that owns all calendar logic, storage and sync,
and a webview that renders the UI. Write the UI in **Svelte 5** (runes mode) with **SvelteKit**
as a static single-page app, in TypeScript.

The split of responsibilities is part of the decision: the frontend renders and collects input,
and everything else lives in Rust. See [architecture.md](../architecture.md).

## Options Considered

Two separate questions were weighed: how to build the application shell, and which UI framework
to use inside a webview.

### Shell, option A: Tauri 2 (chosen)

| Dimension         | Assessment                                                                   |
| ----------------- | ---------------------------------------------------------------------------- |
| Memory and size   | Uses the webview the OS already has, so no browser engine is bundled         |
| UI polish         | Full HTML and CSS: typography, animation and theming are well-trodden ground |
| Logic in Rust     | Yes, with a clear command boundary between UI and core                       |
| Platform coverage | Ubuntu and Windows today, with iOS and Android targets in Tauri 2            |
| Complexity        | Medium: two languages and a boundary between them                            |

**Pros:**

- No bundled browser engine, which fits the low-memory goal.
- The web platform makes the intended look achievable without fighting a toolkit.
- Rust is used where it pays off (parsing, recurrence, sync, SQLite, keychain).
- A permission system limits what the webview may call.

**Cons:**

- Two different webview engines: WebKitGTK on Ubuntu and WebView2 (Chromium) on Windows. CSS and
  JavaScript features differ between them.
- The webview's own processes still use memory. "No bundled engine" is not the same as "small":
  the empty scaffold already idles at about 181 MB (see the baseline under "To revisit").
- Data has to be serialised across the command boundary.
- Linux packaging depends on the system's WebKitGTK version.

### Shell, option B: a native Rust GUI toolkit

Toolkits in this category include egui, iced, Slint and GTK bindings.

| Dimension         | Assessment                                                                        |
| ----------------- | --------------------------------------------------------------------------------- |
| Memory and size   | Best of the options: no webview at all                                            |
| UI polish         | Hardest: text rendering, animation, theming and accessibility need much more work |
| Logic in Rust     | Yes, one language throughout                                                      |
| Platform coverage | Desktop is fine; the mobile story varies per toolkit and is generally weaker      |
| Complexity        | High for this kind of UI: a calendar grid with drag, resize and transitions       |

**Pros:**

- One language, one process, the smallest footprint.
- No differences between webview engines.

**Cons:**

- The design goal (refined typography, subtle animation, light and dark mode) is where these
  toolkits cost the most effort.
- Fewer ready-made answers for UI problems than on the web platform.
- Learning a GUI toolkit and Rust at the same time would slow down the part that matters most,
  which is the calendar logic.

### Frontend, option A: Svelte 5 with SvelteKit (chosen)

| Dimension        | Assessment                                               |
| ---------------- | -------------------------------------------------------- |
| Runtime weight   | Components are compiled; the runtime that ships is small |
| Fit for this app | Good: the UI is a thin view over data that Rust provides |
| Complexity       | Low: little boilerplate, reactivity through runes        |

**Pros:**

- Little framework code runs in the webview, which fits the low-memory goal.
- Scoped styles and built-in transitions cover the design needs without extra libraries.
- SvelteKit with the static adapter gives routing and a build pipeline with no server.

**Cons:**

- Smaller ecosystem than React. Fewer ready-made calendar components, although the plan is to
  build the calendar views in-house anyway.
- Svelte 5 runes are recent, so older examples and some libraries use the previous syntax.
- SvelteKit's server features (`+page.server.ts`, form actions) cannot be used in a static app.

### Frontend, option B: React

| Dimension        | Assessment                                                                             |
| ---------------- | -------------------------------------------------------------------------------------- |
| Runtime weight   | A runtime with a virtual DOM ships with the app                                        |
| Fit for this app | Works, but state libraries and patterns assume more client-side logic than exists here |
| Complexity       | Medium: more decisions to make (routing, state, styling)                               |

**Pros:** the largest ecosystem, many components, the most documentation.
**Cons:** more runtime in the webview, more choices to make and maintain, and its strengths
(ecosystem for complex client-side state) matter little when the logic lives in Rust.

### Frontend, option C: Angular

| Dimension        | Assessment                                                   |
| ---------------- | ------------------------------------------------------------ |
| Runtime weight   | The heaviest of the three                                    |
| Fit for this app | Built for large applications with much client-side structure |
| Complexity       | High for a small UI: modules, dependency injection, RxJS     |

**Pros:** complete and opinionated, strong tooling, good for large teams.
**Cons:** more framework than a thin view layer needs, and the largest footprint in the webview.

### Not evaluated in depth

Electron, Flutter, Qt, .NET-based toolkits and Wails were not formally compared. Electron is the
usual default for web-technology desktop apps; it bundles its own Chromium and Node.js, which
runs against the low-memory goal. This is noted for completeness and is not the result of a
measurement.

## Trade-off Analysis

- **Footprint versus UI effort.** A native toolkit would use the least memory but makes the
  intended look expensive to build. Tauri gives up some footprint (a webview process) to get
  the web platform's UI capabilities. For an app whose value is partly in how it looks, that
  trade is accepted.
- **One language versus two.** Tauri means Rust and TypeScript with a boundary between them.
  The boundary is also an advantage: it forces calendar logic to stay out of the UI, which
  makes it testable without a window and reusable for mobile.
- **Ecosystem versus weight in the frontend.** React and Angular offer more ready-made parts.
  Because the frontend is deliberately thin, those parts are worth less here than a small
  runtime and less boilerplate.
- **Uniform engine versus system engine.** Bundling a browser would behave identically on
  every platform. Using the system webview saves memory and size, and the price is testing on
  two engines.

## Consequences

**Easier:**

- Calendar logic is plain Rust that can be unit-tested without any UI.
- The same core can back a mobile app later.
- The design goals are reachable with ordinary CSS.
- Native integrations (keychain, notifications, SQLite) are available from Rust.

**Harder:**

- Every CSS or JavaScript feature has to work in both WebKitGTK and WebView2, or have a
  fallback. Development on Ubuntu does not show Windows problems, so Windows needs manual
  testing of CI builds.
- Types exist twice, in Rust and in TypeScript, and have to be kept in step.
- Rust's learning curve slows early progress.
- The frontend has no server: only client-side SvelteKit features are available.

**To revisit:**

- If WebKitGTK rendering or performance becomes a real limitation on Ubuntu.
- When mobile work starts: check that Tauri's mobile support and the plugins in use are mature
  enough at that point.
- If keeping Rust and TypeScript types in step by hand becomes error-prone, consider generating
  the TypeScript types.
- **If idle memory moves away from the measured baseline below.** Low memory is the first
  principle of the project, so the number is recorded here to compare against.

### Memory baseline

Measured on 2026-10-13 with the empty scaffold: no calendar code, the SvelteKit starter page in
an 800 by 600 window.

| Process                                          | PSS, MB   |
| ------------------------------------------------ | --------- |
| `simple-calendar` (Rust, GTK, WebKit UI process) | 64.1      |
| `WebKitWebProcess` (renders the page)            | 97.3      |
| `WebKitNetworkProcess`                           | 16.1      |
| Helpers (`bwrap`, `glycin-svg`)                  | 4.0       |
| **Total**                                        | **181.5** |

- Averages of three launches. The totals were 182.9, 180.6 and 181.0 MB.
- **PSS** (proportional set size) counts memory shared with other processes in proportion to
  how many share it, so the values can be added up. For comparison, the summed RSS was 420.9 MB
  (shared pages counted in full for every process, so too high) and the summed private memory
  was 133.6 MB (shared pages not counted at all, so too low).
- Setup: Ubuntu 26.04.1, GNOME on Wayland, WebKitGTK 2.52.6, Tauri 2.12.3, release build from
  `tauri build`.
- Method: start the binary, wait 20 seconds without touching it, then sum `Pss` from
  `/proc/<pid>/smaps_rollup` over the process and all its descendants.

What it says:

- About 113 MB of the 181 MB is in WebKit's own processes before the app does anything. That
  floor comes with the choice of a webview on Linux and is not something application code can
  remove.
- The meaningful number for this project is therefore the **increase over the baseline**: what
  the Rust core, SQLite and the real UI add on top.

When to act:

- Measure again the same way before each release and add the result here.
- An increase that the loaded data does not explain is a reason to investigate before
  shipping.
- If the floor itself turns out to be too high for what "low memory first" is meant to achieve,
  that is the argument for reopening this decision in favour of a native toolkit.
- Not measured yet: Windows with WebView2. Do this at the first manual Windows test, since the
  two engines will not have the same footprint.

## Action Items

1. [x] Scaffold Tauri 2 with SvelteKit (static adapter, SSR off, runes mode).
2. [x] CI that builds on Ubuntu and Windows and uploads the Windows installer.
3. [ ] Keep all calendar logic in the `simplecalendar-core` crate, which has no Tauri dependency
       (see [architecture.md](../architecture.md), section 3).
4. [ ] Measure the memory baseline on Windows and add it under "To revisit".
