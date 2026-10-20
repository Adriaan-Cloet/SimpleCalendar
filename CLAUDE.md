# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# SimpleCalendar

Lightweight, local-first desktop calendar. Tauri 2 + Rust core + Svelte 5 + TypeScript. Package manager is npm.

Current state: scaffold plus a static week-view design prototype on fake data (`src/routes/+page.svelte`, data in `src/lib/prototype/`). SQLite storage exists (`src-tauri/crates/core/src/db/`: two connections, migrations, the v0.1 schema), and the app opens it at startup. Reading a date range works in the core (`db/occurrences/`: recurrence expansion in `recurrence/`, model in `model/`), but no Tauri command calls it yet. Secrets go through `secrets::Secrets` (OS keychain, probed at startup). The provider traits are in `provider/` and the sync engine in `sync/`: it starts with the app, syncs on start, on a timer and on `sync_now`, and the app crate forwards its `CoreEvent`s as `calendar:changed` and `sync:status`. No provider is registered yet (the ICS provider is #7), so a sync finds nothing to do. Tauri commands: `get_status`, `sync_now`. The plan is in `docs/roadmap.md` (versions v0.1 to v1.0, one GitHub milestone each) and `docs/architecture.md`.

## Scope

- Target platforms: Ubuntu and Windows (desktop only for now, mobile later).
- Calendar sources: ICS subscriptions (read-only), Google Calendar API (read/write).
  CalDAV comes in v0.4: design the provider trait for it from day one.
- Users must be able to create, edit and delete events, not only subscribe.

## Principles

- Low memory first: no heavy deps without asking. Justify every new crate/package.
- All calendar logic (parsing, recurrence, sync) lives in Rust, not in the frontend.
- Local SQLite is the source for the UI; writes go through an outbox queue.
- Never store tokens in plain files or SQLite: use the OS keychain.
- Design: minimal, Apple-like, generous whitespace, system font, subtle animations, light + dark mode.
- Development happens on Ubuntu (WebKitGTK). Windows (WebView2) is built in CI and tested manually.
  Avoid CSS/JS features that differ between both webviews without a fallback.

## Workflow

- Look up current Tauri 2 / Svelte 5 docs via context7 before using APIs.
- Explain non-obvious Rust to me; I'm learning Rust.
- Write tests for recurrence/timezone logic before implementing.
- Before finishing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run check`.
- Conventional commits (`feat:`, `fix:`, `docs:`, `chore:`...).

## Commands

```sh
npm run tauri dev    # run the desktop app with hot reload (starts Vite, then the Rust binary)
npm run tauri build  # release bundle (AppImage target needs `patchelf` installed)
npm run dev          # frontend only, in a browser at http://localhost:5173
npm run check        # svelte-check (Svelte + TypeScript types)
npm run lint         # prettier --check + eslint
npm run format       # prettier --write
```

- `cargo` commands run from `src-tauri/`. Pass `--workspace` (`--all` for `cargo fmt`), or only the app crate is checked. A single Rust test: `cargo test -p simplecalendar-core <name>`.
- `scripts/test-keychain.sh` runs the opt-in tests against a real Secret Service, in throwaway D-Bus sessions (Linux; needs `dbus-run-session` and `gnome-keyring-daemon`).
- The frontend has no test runner.
- If `cargo` is not on PATH in a non-interactive shell, run `. ~/.cargo/env` first; `npm run tauri dev` fails without it.
- CI (`.github/workflows/ci.yml`) runs all checks plus a Tauri build on Ubuntu and Windows, and uploads the Windows installer and `.exe` as a workflow artifact.

## Architecture

- **Two halves, one window.** `src/` is the SvelteKit frontend, `src-tauri/` the Rust shell. In dev, `tauri dev` starts Vite and opens a native window on its URL. For a release, `tauri build` embeds the static `build/` folder in the binary. The wiring is in `src-tauri/tauri.conf.json`.
- **The dev port is pinned in two places:** `port: 5173` with `strictPort` in `vite.config.ts`, and `devUrl` in `tauri.conf.json`. Change both together.
- **The frontend is an SPA with no server.** `src/routes/+layout.ts` sets `ssr = false` and the static adapter falls back to `index.html`. Server-only SvelteKit features (`+page.server.ts`, `+server.ts`, form actions) do not work.
- **SvelteKit 3 keeps its config in `vite.config.ts`.** There is no `svelte.config.js`. Runes mode is forced for all project files.
- **Shared frontend code** lives in `src/lib/` and is imported as `#lib/...` (declared in `package.json`), not `$lib`.
- **Frontend to Rust:** the frontend calls `invoke` from `@tauri-apps/api/core`; the Rust side is a `#[tauri::command]` function registered on the builder in `src-tauri/src/lib.rs`. `main.rs` only calls `simple_calendar_lib::run()`.
- **Rust workspace.** `src-tauri/Cargo.toml` is both the app crate and the workspace root, so `Cargo.lock` and `target/` stay in `src-tauri/`. All calendar logic goes in `src-tauri/crates/core` (package `simplecalendar-core`, imported as `simplecalendar_core`). It must never depend on `tauri`; the app crate only wraps it in commands.
- **Storage.** `simplecalendar_core::db::Db` holds one write and one read-only connection. Every call goes through `Db::write` / `Db::read`, which run the closure in `spawn_blocking`. Schema changes are a new numbered file in `db/migrations/`, added to `MIGRATION_SLICE`; never edit a shipped migration. Query tests start from `db::migrations::test_connection()` (in-memory, all migrations applied).
- **Permissions:** `src-tauri/capabilities/default.json` controls which Tauri APIs and plugins the frontend may use. A new plugin needs its permission added there or calls are rejected at runtime.

## Conventions

- Colours, type, spacing, radii, shadows and durations come from the tokens in `src/lib/styles/tokens.css`, never raw values. `docs/design.md` documents them and the rules for looking the same in WebKitGTK and WebView2.
- Import `.ts` files from `src/lib/` with a `.js` extension: `#lib/calendar-color.js`. Without it the `#lib/*` mapping does not resolve.
- Prettier: tabs, single quotes, no trailing commas, width 100. It also formats the JSON under `src-tauri/`.
- The root `.gitignore` is the source of truth for ignores; ESLint reads it directly. `src-tauri/Cargo.lock` is committed.
- The bundle identifier `com.adriaancloet.simplecalendar` determines the app-data directory, so changing it orphans existing user data.
