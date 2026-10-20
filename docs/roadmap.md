# Roadmap

Status: in progress. Done: #2, #3, #4, #5, #6, #7.
Last updated: 2026-10-20.

This document splits v1 into versions. Each version is a GitHub milestone, and each feature in it
is an issue with acceptance criteria. How the features are built is described in
[architecture.md](architecture.md); the section numbers below (§) refer to that document.

## How to read it

- Versions have no dates. There is one developer and no deadline (see
  [ADR 0001](adr/0001-tauri-svelte.md)).
- A version is done when every issue in its milestone is closed and its exit criteria hold.
- The order inside a version is the order of the issues. Later issues depend on earlier ones;
  each issue lists its dependencies.
- Every issue is finished only when `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`
  and `npm run check` pass, as `CLAUDE.md` requires.

## Features and versions

The architecture marks each section with the feature that first needs it. This table maps those
features to versions.

| Feature           | Version | What it adds                                                        |
| ----------------- | ------- | ------------------------------------------------------------------- |
| ICS subscriptions | v0.1    | Read-only feeds, and with them the storage, sync engine and secrets |
| local calendars   | v0.2    | Calendars that live only in SQLite, with create, edit and delete    |
| Google read       | v0.2    | Sign-in and syncing Google calendars into SQLite                    |
| Google write      | v0.2    | Creating, editing and deleting Google events through the outbox     |
| offline sync      | v0.3    | Editing without a connection, retries, conflicts, sync issues       |
| reminders         | v0.3    | Desktop notifications                                               |
| CalDAV            | v0.4    | CalDAV accounts, read and write                                     |

The week view in v0.1 already needs SQLite, recurrence and the frontend contract, so those parts
arrive with ICS subscriptions, before local calendars.

## Overview

| Version | Theme                                    | Issues |
| ------- | ---------------------------------------- | ------ |
| v0.1    | ICS subscriptions and week view          | 11     |
| v0.2    | Google read/write and local calendars    | 6      |
| v0.3    | Offline outbox, sync, day and month view | 7      |
| v0.4    | CalDAV                                   | 3      |
| v1.0    | Google verification and installers       | 5      |

## v0.1: ICS subscriptions and week view

Goal: subscribe to read-only ICS feeds and see them in a real week view. This version lays the
foundation everything else builds on: the core crate, SQLite, recurrence, the sync engine and
the keychain.

| Issue                                                            | Feature                                                    |
| ---------------------------------------------------------------- | ---------------------------------------------------------- |
| [#2](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/2)   | Set up the `simplecalendar-core` crate and Cargo workspace |
| [#3](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/3)   | SQLite storage: two connections, migrations, base schema   |
| [#4](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/4)   | Recurrence expansion and time zones (tests first)          |
| [#5](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/5)   | Keychain secret store                                      |
| [#6](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/6)   | Provider trait and sync engine for read-only sources       |
| [#7](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/7)   | ICS provider: fetch and parse feeds                        |
| [#8](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/8)   | ICS time-zone resolution (tests first)                     |
| [#9](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/9)   | Subscribe to and manage ICS feeds                          |
| [#10](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/10) | Overlap layout function with Vitest (tests first)          |
| [#11](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/11) | Week view on real data                                     |
| [#12](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/12) | Calendar sidebar: visibility and colour                    |

Exit criteria:

- A public feed and a private one (Google's "secret address in iCal format") can be added,
  show up in the week view and refresh on the timer and on demand.
- All 22 ICS time-zone cases of §12 pass.
- The feed URL exists only in the keychain. Without a keyring the app starts and says why
  subscribing is blocked.
- The prototype's fake data in `src/lib/prototype/` is gone.
- The CI build of the Windows installer has been tried by hand.

Not in this version: creating or editing events, Google, day and month views.

## v0.2: Google read/write and local calendars

Goal: sign in with Google, see its calendars, and create, edit and delete events, both in Google
calendars and in a local calendar that needs no account.

| Issue                                                            | Feature                                             |
| ---------------------------------------------------------------- | --------------------------------------------------- |
| [#13](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/13) | Google sign-in with OAuth and PKCE                  |
| [#14](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/14) | Google provider: read calendars and events          |
| [#15](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/15) | Local calendars                                     |
| [#16](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/16) | Event editor: create, edit, delete                  |
| [#17](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/17) | Recurring edits: "only this event" and "all events" |
| [#18](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/18) | Basic outbox and Google write                       |

Exit criteria:

- Several Google accounts can be added. Their calendars sync incrementally.
- An edit made in the app appears in Google Calendar on the web, and the other way round.
- An edit never erases fields the app does not store (guests, conferencing, attachments).
- Events you may not change are read-only in the UI and rejected by the core.
- A local calendar works without any account and without a network.

Not in this version: editing while offline, retries with backoff, conflict handling beyond
"never overwrite without an ETag".

## v0.3: Offline outbox, sync, day and month view

Goal: the app keeps working without a connection and catches up on its own. Day and month views
and reminders complete the everyday features.

| Issue                                                            | Feature                             |
| ---------------------------------------------------------------- | ----------------------------------- |
| [#19](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/19) | Outbox: offline robustness          |
| [#20](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/20) | Server-wins conflict handling       |
| [#21](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/21) | Sync issues list                    |
| [#22](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/22) | Sync triggers and status            |
| [#23](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/23) | Day view                            |
| [#24](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/24) | Month view                          |
| [#25](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/25) | Reminders and desktop notifications |

Exit criteria:

- Edits made with the network off show immediately and reach Google once it is back, in order.
- Killing the app during a push loses nothing and creates no duplicate.
- A change made on another device at the same time is never silently lost (§7.4).
- Week, day and month views share the same data and navigation.
- A reminder fires once, also across a restart.

Not in this version: asking the user to resolve a conflict (server wins), background sync with
the window closed.

## v0.4: CalDAV

Goal: CalDAV accounts as a third provider, read and write, on the trait designed for it (§5).

| Issue                                                            | Feature                             |
| ---------------------------------------------------------------- | ----------------------------------- |
| [#26](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/26) | CalDAV provider: discovery and read |
| [#27](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/27) | CalDAV write                        |
| [#28](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/28) | Add a CalDAV account                |

Exit criteria:

- Tested against at least two servers, for example Radicale and Nextcloud.
- No change to `CalendarProvider` or `WritableProvider` was needed. If one was, the reason is
  recorded in the architecture.
- The password is stored only in the keychain.

## v1.0: Google verification and installers

Goal: an app other people can install and sign in to without warnings or weekly re-sign-ins.

| Issue                                                            | Feature                                              |
| ---------------------------------------------------------------- | ---------------------------------------------------- |
| [#29](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/29) | Google OAuth verification                            |
| [#30](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/30) | Ubuntu packages: .deb and AppImage                   |
| [#31](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/31) | Windows installer                                    |
| [#32](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/32) | Release workflow                                     |
| [#33](https://github.com/Adriaan-Cloet/SimpleCalendar/issues/33) | Memory measurement before release, including Windows |

Exit criteria:

- The Google consent screen is verified and in production, so refresh tokens no longer expire
  after 7 days.
- Pushing a tag `v1.0.0` produces a GitHub Release with the Ubuntu and Windows installers.
- Both installers were tested on a clean machine.
- Idle memory on both platforms is recorded in ADR 0001 and explained against the baseline.

## After v1

No milestone yet. Each is described in the architecture, which keeps the design open for it.

| Feature                              | Where                     |
| ------------------------------------ | ------------------------- |
| AI access through an MCP server      | §14                       |
| "This and following events"          | §1                        |
| Attendees, invitations, RSVP         | §1, §13 point 10          |
| Tray icon and background sync        | §1                        |
| Asking the user to resolve conflicts | §1, §7.4                  |
| Mobile                               | §1, ADR 0001 "To revisit" |
