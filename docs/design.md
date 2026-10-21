# Design system

Status: first version. The week view runs on real data since #11.
Last updated: 2026-10-21 (event editor, §11, §13).

This document describes how SimpleCalendar looks and moves. The tokens live in
`src/lib/styles/tokens.css`, the global base styles in `src/lib/styles/base.css`. The prototype in
`src/routes/+page.svelte` uses both and is the reference for how the pieces combine.

## 1. Principles

- **Content first.** The window chrome is white and grey. Calendar colours are the only
  saturated colour on screen, so events are what the eye finds first.
- **One accent.** Blue marks what you can interact with (the primary button, focus rings,
  selected controls). Red means "today", "now" or "destructive", and nothing else.
- **Hierarchy through type and grey, not boxes.** Use weight and the three text greys before
  adding a border, a card or a background.
- **Calm motion.** Motion answers what the user did. Nothing animates on load.
- **Tokens only.** Colours, type, spacing, radii, shadows and durations come from `tokens.css`.
  A value that is missing becomes a token first. Sizes that belong to one component, such as
  the diameter of the today badge, may stay in that component.
- **Same look on both webviews.** Ubuntu (WebKitGTK) and Windows (WebView2) differ in fonts,
  scrollbars and compositing. §10 lists the rules that keep the result the same.

## 2. Colour: neutrals

| Token                    | Light                   | Dark                    | Use                                                                       |
| ------------------------ | ----------------------- | ----------------------- | ------------------------------------------------------------------------- |
| `--text`                 | `#1D1D1F`               | `#F5F5F7`               | Body text, titles                                                         |
| `--text-secondary`       | `#6E6E73`               | `#98989D`               | Labels, hour labels, metadata                                             |
| `--text-tertiary`        | `#A1A1A6`               | `#6E6E73`               | Decorative only: days outside the month, icons. No text that must be read |
| `--surface`              | `#FFFFFF`               | `#1C1C1E`               | Main background, week grid                                                |
| `--surface-weekend`      | `#FAFAFB`               | `#202022`               | Weekend columns                                                           |
| `--surface-sidebar`      | `#F5F5F7`               | `#232325`               | Sidebar                                                                   |
| `--surface-elevated`     | `#FFFFFF`               | `#2C2C2E`               | Popovers, dialogs                                                         |
| `--fill-hover`           | black 4%                | white 6%                | Hover on plain buttons and rows                                           |
| `--fill-pressed`         | black 8%                | white 10%               | Pressed state, current week in the mini month                             |
| `--fill-control`         | `rgba(118,118,128,.12)` | `rgba(118,118,128,.24)` | Track of a segmented control                                              |
| `--control-thumb`        | `#FFFFFF`               | `#636366`               | Selected segment                                                          |
| `--separator`            | black 10%               | white 12%               | Borders between regions                                                   |
| `--grid-line`            | black 7%                | white 8%                | Hour lines and day lines                                                  |
| `--backdrop`             | black 20%               | black 50%               | Behind a modal dialog                                                     |
| `--accent`               | `#0068D6`               | `#0A6CDB`               | Fills: primary button, checked control, focus ring                        |
| `--accent-text`          | `#0066CC`               | `#3B9BFF`               | Blue text, such as links                                                  |
| `--now`                  | `#D92D20`               | `#D92D20`               | Today badge, now line                                                     |
| `--now-text`, `--danger` | `#D92D20`               | `#FF6961`               | Red text: today's weekday, the now label, "Delete"                        |

Why these values and not Apple's: `#007AFF` reaches only 4.0:1 as text on white, and white text
on `#FF3B30` only 3.5:1. Both fail WCAG AA (4.5:1) at our text sizes. The values above are the
nearest that pass. In dark mode no single blue works both as text on the background and as a
fill behind white text, which is why the accent has two tokens.

Measured contrast (WCAG): `--text-secondary` on `--surface` 5.1:1 light and 5.9:1 dark, on the
elevated surface 4.9:1 dark. White on `--accent` 5.3:1 light and 5.0:1 dark. White on `--now`
4.8:1.

## 3. Colour: calendars

### Palette

Ten colours, offered when the user picks a calendar colour. Each has a light and a dark value.

| Name   | Light     | Dark      |
| ------ | --------- | --------- |
| red    | `#FF3B30` | `#FF453A` |
| orange | `#FF9500` | `#FF9F0A` |
| yellow | `#FFCC00` | `#FFD60A` |
| green  | `#34C759` | `#30D158` |
| teal   | `#30B0C7` | `#40CBE0` |
| blue   | `#007AFF` | `#0A84FF` |
| indigo | `#5856D6` | `#5E5CE6` |
| purple | `#AF52DE` | `#BF5AF2` |
| pink   | `#FF2D55` | `#FF375F` |
| brown  | `#A2845E` | `#AC8E68` |

The database stores a calendar colour as `#rrggbb` (architecture §6). A colour that matches a
light palette value is drawn through its token (`--cal-blue`), so it switches in dark mode. Any
other value, such as a colour Google sends, is used as is in both modes
(`src/lib/calendar-color.ts`).

### Drawing an event

Every event is drawn from one variable, `--cal`, so a palette colour and an arbitrary provider
colour go through the same rules:

| Part           | CSS                                                                       | Light | Dark |
| -------------- | ------------------------------------------------------------------------- | ----- | ---- |
| Fill           | `color-mix(in oklab, var(--cal) var(--event-fill-mix), var(--surface))`   | 14%   | 24%  |
| Fill, hover    | same with `--event-hover-mix`                                             | 20%   | 32%  |
| Fill, selected | same with `--event-selected-mix`, plus a 1px ring in `--cal`              | 28%   | 40%  |
| Fill, raised   | same with `--event-raised-mix`, for an event that lies over another (§11) | 23%   | 33%  |
| Text           | `color-mix(in oklab, var(--cal) 45%, var(--text))`                        | 45%   | 45%  |

The text mixes towards `--text`, so it gets darker in light mode and lighter in dark mode
without a second rule. Selected events get a stronger fill and a ring rather than a solid fill
with white text, because white on yellow or teal is unreadable.

Measured: the text reaches at least 4.86:1 on every fill state of all ten palette colours in
both modes, including on weekend columns (worst case: yellow, selected). Five common Google
colours were checked as well and reach at least 5.2:1. A colour change in the palette or in the
mix percentages needs this check again.

There is no coloured bar along the edge of an event: the fill and the text colour already say
which calendar it belongs to. All-day events use the same fill and text.

## 4. Typography

The app uses the operating system's UI font. There is no web font to download or bundle.

```css
--font-ui:
	'Segoe UI Variable Text', 'Segoe UI', system-ui, -apple-system, 'Ubuntu Sans', Ubuntu, Cantarell,
	'Noto Sans', sans-serif;
--font-display: 'Segoe UI Variable Display', /* then the same list */;
```

- Windows 11 gets Segoe UI Variable, with the Display cut for large text. Windows 10 gets
  Segoe UI.
- Ubuntu has no Segoe fonts, so `system-ui` applies. In WebKitGTK that is the GNOME interface
  font (Ubuntu Sans on Ubuntu 26.04). The named Linux fonts after it are a fallback for
  engines that do not resolve `system-ui`.

| Token             | Size / line height | Weight                     | Use                                              |
| ----------------- | ------------------ | -------------------------- | ------------------------------------------------ |
| `--text-caption`  | 11 / 14            | 400                        | Hour labels, event times, compact event titles   |
| `--text-footnote` | 12 / 16            | 400, 600                   | Event titles, group headings, segmented controls |
| `--text-body`     | 13 / 18            | 400, 500                   | Sidebar, popover body, buttons                   |
| `--text-headline` | 15 / 20            | 600                        | Popover title                                    |
| `--text-title`    | 22 / 28            | 600, display font, −0.01em | Month heading in the toolbar                     |

Rules:

- Weights are 400, 500 and 600 only. Nothing below 400: thin weights break up under ClearType
  on Windows.
- Times and dates use tabular numerals (`.tabular`), so columns of times line up.
- Sentence case everywhere. No all-caps labels.
- Text that can be long (event titles, calendar names) is one line with an ellipsis, unless the
  space is known to be large enough for more.

## 5. Spacing and layout metrics

A 4px base:

| Token         | Value |
| ------------- | ----- |
| `--space-2xs` | 2px   |
| `--space-xs`  | 4px   |
| `--space-sm`  | 8px   |
| `--space-md`  | 12px  |
| `--space-lg`  | 16px  |
| `--space-xl`  | 24px  |
| `--space-2xl` | 32px  |
| `--space-3xl` | 48px  |

| Token              | Value | Why                                                                                   |
| ------------------ | ----- | ------------------------------------------------------------------------------------- |
| `--hour-height`    | 60px  | One pixel per minute. Event edges land on whole pixels, also at 125% and 150% scaling |
| `--gutter-width`   | 56px  | Fits "23:00" in caption size with room to spare                                       |
| `--sidebar-width`  | 232px | Mini month at a comfortable size; hidden below 900px window width                     |
| `--toolbar-height` | 52px  |                                                                                       |
| `--control-height` | 28px  | Buttons, segmented controls, sidebar rows                                             |

The window opens maximized, not in fullscreen, so the title bar and the desktop's panels stay
visible. Restored, it is 1200 × 800 and centred; it cannot get smaller than 760 × 520
(`tauri.conf.json`).

## 6. Radii

The radius grows with the size and level of the element. Elements of one kind always share a
radius.

| Token           | Value | Use                                                     |
| --------------- | ----- | ------------------------------------------------------- |
| `--radius-sm`   | 4px   | Events, checkboxes, segments inside a segmented control |
| `--radius-md`   | 6px   | Buttons, segmented controls, sidebar rows               |
| `--radius-lg`   | 12px  | Popovers, dialogs                                       |
| `--radius-full` | 999px | Today badge, dots                                       |

## 7. Shadows

Three levels. Each is a soft shadow plus a 1px ring, so the edge stays crisp on any background.

| Token        | Use                                  |
| ------------ | ------------------------------------ |
| `--shadow-1` | Raised control: the selected segment |
| `--shadow-2` | Popovers, menus                      |
| `--shadow-3` | Dialogs, an event being dragged      |

In dark mode a shadow barely shows against a dark background. The dark values therefore use
higher opacity plus an inner `rgba(255,255,255,.08)` hairline, and elevated elements also use
the lighter `--surface-elevated`.

Nothing in the week grid has a shadow. Depth is reserved for things that float above it.

## 8. Motion

| Token             | Value                        | Use                                         |
| ----------------- | ---------------------------- | ------------------------------------------- |
| `--duration-fast` | 120ms                        | Hover, press, checkbox                      |
| `--duration-base` | 200ms                        | Fades, such as showing or hiding a calendar |
| `--duration-slow` | 320ms                        | Popovers, sheets                            |
| `--ease-standard` | `cubic-bezier(0.2, 0, 0, 1)` | Things that appear or change                |
| `--ease-exit`     | `cubic-bezier(0.4, 0, 1, 1)` | Things that leave                           |

- Only `opacity` and `transform` animate. Both are cheap in both engines, also when WebKitGTK
  runs without GPU compositing.
- A popover grows from 96% to 100% scale while fading in, from the side that faces its event.
- Hiding a calendar fades its events out. They keep their place, so nothing else jumps, until
  the user moves to another week.
- `prefers-reduced-motion: reduce` sets all durations to `0ms`, in one place in `tokens.css`.
  WebKitGTK reports it when GNOME animations are off, WebView2 when Windows animations are off.

## 9. Light and dark

- Light values are on `:root`. The dark values are in two identical blocks: under
  `@media (prefers-color-scheme: dark)` for `:root:not([data-theme='light'])`, and under
  `:root[data-theme='dark']`.
- No `data-theme` attribute means "follow the system". Setting it to `light` or `dark` forces
  that theme. The prototype's appearance control does this: three icon-only segments (half-filled circle for
  system, sun, moon), each with a tooltip and an accessible name. In the app it becomes a setting.
- Each theme sets `color-scheme`, so native scrollbars, form controls and the default
  background follow the theme.
- `light-dark()` is not used: it needs a fallback block for older engines, which would bring
  back the duplication it is meant to remove.

How "follow the system" reaches the page:

- **Ubuntu:** tao (Tauri's windowing layer, built with its `dbus` feature) reads the desktop's
  `color-scheme` through the XDG portal and sets GTK's prefer-dark flag. WebKitGTK reports that
  flag as `prefers-color-scheme: dark`. Checked on 2026-10-14 against WebKitGTK 2.52 and tao
  0.37.
- **Windows:** WebView2 follows the Windows app mode.

## 10. Two webviews

Development happens in WebKitGTK; Windows is only tested by hand. These rules exist because the
two engines differ.

| Rule                                                                                                                           | Reason                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| No `backdrop-filter`. Use opaque surfaces                                                                                      | WebKitGTK may run without GPU compositing (common on NVIDIA). Blur then becomes slow or disappears, and it costs memory  |
| Hairlines are 1px with a transparent colour, never 0.5px                                                                       | The engines round half pixels differently                                                                                |
| Do not rely on `-webkit-font-smoothing`                                                                                        | It only works on macOS                                                                                                   |
| Style scrollbars only with `::-webkit-scrollbar`. Never set `scrollbar-width` or `scrollbar-color`                             | Both engines support the pseudo-elements. Chromium ignores them as soon as one of the standard properties is set         |
| Never align two scroll containers by assuming a scrollbar width. Put fixed headers inside the scroller with `position: sticky` | Scrollbar widths differ per engine and setting. The week view does this for the day header and all-day row               |
| Text never depends on an exact width. Long text ellipsizes                                                                     | Ubuntu Sans and Segoe UI have different widths                                                                           |
| `user-select` also gets the `-webkit-` prefix                                                                                  | WebKitGTK still needs it                                                                                                 |
| Icons are inline SVG drawn with `currentColor`. No emoji, no icon font                                                         | Emoji come from different fonts (Noto Color Emoji, Segoe UI Emoji) and look different. An icon font is one more download |
| Animate only `opacity` and `transform`                                                                                         | See §8                                                                                                                   |
| `color-mix()` is the newest feature in use (WebKitGTK 2.40+, Chromium 111+). Anything newer needs a fallback                   | Ubuntu LTS gets WebKitGTK updates, WebView2 is evergreen, but a newer feature can still be missing on an older system    |

Checked so far: the prototype renders the same layout in WebKitGTK 2.52 and in Chromium on
Linux, in both themes and at 760 × 520, and in the Tauri app itself on Ubuntu (Wayland and
XWayland, window maximized). Chromium on Linux is a stand-in only: the real WebView2 check is
the Windows build from CI.

## 11. Week view

```
┌ sidebar ─────────┬ October 2026          [Day|Week|Month]        ‹ Today ›   + ┐
│ mini month       ├─────┬ Mon 12 ┬ Tue 13 ┬ Wed (14) ┬ Thu 15 ┬ … ┬ Sun 18 ──────┤
│ (this week band) │all- │ Mum's  │ Conference, Ghent ─────────┐ │   │           │
│                  │day  ├────────┼────────┼──────────┼────────┼───┼─────────────┤
│ On this computer │09:00│ Standup│ Standup│ Standup  │        │   │ weekend     │
│ ■ Personal       │     │        │ Design  1:1      │        │   │ columns     │
│ Google           │13:00│        │        │●━━━━━━━━━│ now    │   │ tinted      │
│ ■ Work           │     │        │        │ Workshop │        │   │             │
│ Subscriptions    │     │  the grid scrolls; the header and all-day row stick   │
│ ■ Club fixtures  │                                                               │
│ [◐|sun|moon]     │                                                               │
└──────────────────┴───────────────────────────────────────────────────────────────┘
```

| Element             | Look                                                                                                           |
| ------------------- | -------------------------------------------------------------------------------------------------------------- |
| Toolbar             | Month heading left, view switcher centred, navigation and "New event" right                                    |
| Day header          | Short weekday in secondary grey plus the date. Today: weekday in red, date in a red badge with white text      |
| All-day row         | Events span their days; the label "all-day" sits in the gutter                                                 |
| Grid                | Hour lines and day lines in `--grid-line`. Weekend columns in `--surface-weekend`. Opens scrolled to 07:30     |
| Now line            | 2px red line with a dot, only across today's column. Its time label in the gutter replaces a nearby hour label |
| Event, 45 min+      | Title (footnote, semibold), start time, location if there is room                                              |
| Event, under 45 min | One line: title first, then the start time with whatever room is left                                          |
| Overlapping events  | Side by side if they start within 20 minutes; a later one stacks on top, indented. See below                   |
| Selected event      | Stronger fill and a ring; a popover opens beside it                                                            |
| Read-only event     | Looks like any other event in the grid. The popover says why it cannot be edited, the sidebar shows a lock     |
| Popover             | Title, location, date and time, calendar, notes, then "Delete" (left, red) and "Edit" (right, accent)          |
| Delete              | Asks again in the popover footer: "Delete this event?" with "Cancel" and a red "Delete"                        |
| Double click        | On an empty spot: a new event at that half hour; on the all-day row an all-day event                           |

### Overlapping events

Decided in #10, after comparing equal columns, Apple's cascade and this rule drawn at real
size. Equal columns leave three overlapping events about 30px each at the minimum window width.
Apple's cascade keeps the width, but a later event can cover an earlier one's title, and two
events of one colour merge into one shape.

```
Start within 20 min: side by side      Start later: stacked on top
09:00 ┌────────┐┌────────┐              09:00 ┌──────────────────┐
      │ Call   ││ Sync   │                    │ Workshop         │
09:30 │        ││        │              09:30 │    ┏━━━━━━━━━━━━━┓
      │        ││        │                    │    ┃ Review      ┃
10:00 └────────┘└────────┘              10:00 └────┃  (stronger  ┃
                                                   ┃   tint)     ┃
                                                   ┗━━━━━━━━━━━━━┛
```

| Rule              | Value                                                                                      |
| ----------------- | ------------------------------------------------------------------------------------------ |
| Side by side      | When two overlapping events start less than 20 minutes apart (one title line at 1px/min)   |
| Stacked           | Otherwise: shifted `--space-md` per level, drawn on top, at most 3 deep, then a new column |
| Free column first | An event takes a free column before it stacks                                              |
| Widening          | An event spans the columns to its right that are free for its whole length                 |
| Minimum height    | 15 minutes; overlaps are judged on the drawn size                                          |
| Channel           | An event that overlaps another gets a 2px ring in the day's background colour              |
| Stronger tint     | An event that starts later than one it overlaps uses `--event-raised-mix`                  |

The channel and the tint together keep events of one colour apart without lines, shadows or
an edge stripe: the channel separates events side by side, the tint marks the one that lies
on top. The tint sits between the hover and selected fills, which are measured for contrast
(§3), so the text stays readable. Hovering a raised event shows the selected fill.

## 12. Subscriptions dialog

Opened from "Subscriptions" in the sidebar footer, next to the appearance control. A modal
`<dialog>`: the browser handles focus, Escape and the backdrop in both webviews.

| Element         | Look                                                                                                    |
| --------------- | ------------------------------------------------------------------------------------------------------- |
| Dialog          | 480px wide, `--surface-elevated`, `--radius-lg`, `--shadow-3`; grows from 96% while fading in           |
| Source row      | Name (medium), then host and status in footnote grey; warnings below it. Rows separated by hairlines    |
| Status          | "Syncing…", "Updated 5 minutes ago", "Not synced yet", or the last error in `--danger`                  |
| Row actions     | Refresh icon (disabled while syncing) and a quiet "Remove", which asks again: "Cancel" and red "Remove" |
| Keyring problem | The message from architecture §8 on a `--fill-hover` panel, with "Try again". Subscribing is disabled   |
| Add form        | Feed address and an optional name, a note that the address goes to the keyring, primary "Subscribe"     |

The host is shown, never the address: a private feed address is a secret.

## 13. Event editor

Opened by "New event" in the toolbar, a double click on the grid, or "Edit" in the popover
(#16). A modal `<dialog>` like the Accounts dialog.

```
┌ Edit event ─────────────────────────────┐
│ Dentist                     (headline)  │
│ Ghent                       (secondary) │
├─────────────────────────────────────────┤
│ All-day     ( ○)                        │
│ Starts      [2026-10-20 ▾] [09:00]      │
│ Ends        [2026-10-20 ▾] [10:00]      │
│ Time zone   [Europe/Brussels       ▾]   │
├─────────────────────────────────────────┤
│ Calendar    ● [Personal            ▾]   │
│ Reminders   [30 minutes before ▾]  ×    │
│             Add reminder                │
├─────────────────────────────────────────┤
│ Notes…                                  │
│                         Cancel   Save   │
└─────────────────────────────────────────┘
```

| Element         | Look                                                                                                       |
| --------------- | ---------------------------------------------------------------------------------------------------------- |
| Dialog          | 440px wide, otherwise as the Accounts dialog (§12)                                                         |
| Title, location | Borderless text on the dialog, headline and secondary grey, so the event reads like a note, not a form     |
| Rows            | An 88px label column in secondary grey, controls on the right. Groups separated by hairlines               |
| All-day         | A switch: `--fill-control` track, `--accent` when on, the knob moves with `transform`                      |
| Dates and times | Native `date` and `time` inputs. Without a picker (older WebKitGTK) they are text fields with a hint       |
| Time zone       | Only for timed events. The zones the webview knows, plus the event's and the viewer's                      |
| Calendar        | Writable calendars only, with the calendar's colour as a dot                                               |
| Reminders       | One menu per reminder with a remove button; "Add reminder" below. Google email reminders say "by email"    |
| Problems        | An end before the start disables "Save" and says so in `--danger`; errors from Rust show in the same place |
| Footer          | Quiet "Cancel", primary "Add" for a new event or "Save"                                                    |

The sidebar's "On this computer" group has a "+" that adds a local calendar by name, in place.
The calendar menu (right click) adds "Rename…" and a red "Delete calendar…" for local
calendars; deleting asks again below the row, because it takes the events with it.

## 14. Open points

1. **Locale.** The gutter always shows a 24-hour clock ("08:00"). Event times, dates and the
   popover follow the system locale through `Intl.DateTimeFormat`, so on an English system
   they still show "9:00 AM". Decide whether the 24-hour choice becomes a setting for all times.
2. **Sidebar toggle.** Below 900px the sidebar is simply hidden. The app needs a button to show
   it. Until then "Subscriptions" in the sidebar footer cannot be reached in a narrow window.
3. **Dark mode in the real app.** The engine behaviour is verified (§9). Still to confirm by hand:
   switching Ubuntu's style while the app runs, and Windows app mode in the CI build.
