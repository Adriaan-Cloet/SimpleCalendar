<script lang="ts">
	import {
		createEvent,
		createLocalCalendar,
		deleteEvent,
		removeCalendar,
		displayZone,
		getEvent,
		listCalendars,
		listOccurrences,
		occurrenceKey,
		onCalendarChanged,
		updateCalendar,
		updateEvent,
		type CalendarUpdate,
		type CalendarInfo,
		type EventDetail,
		type Occurrence,
		type OccurrenceRef,
		type Scope
	} from '#lib/calendar.js';
	import EventEditor, { NEW_LOCAL_CALENDAR } from '#lib/components/EventEditor.svelte';
	import EventPopover, { type EventDetails } from '#lib/components/EventPopover.svelte';
	import Sidebar from '#lib/components/Sidebar.svelte';
	import SubscriptionsDialog from '#lib/components/SubscriptionsDialog.svelte';
	import SyncIssuesDialog from '#lib/components/SyncIssuesDialog.svelte';
	import SyncNotice from '#lib/components/SyncNotice.svelte';
	import Toolbar from '#lib/components/Toolbar.svelte';
	import WeekView from '#lib/components/WeekView.svelte';
	import { displayLocale } from '#lib/display-format.js';
	import { keepHidden } from '#lib/keep-hidden.js';
	import {
		defaultStart,
		draftFromForm,
		formFromEvent,
		newEventForm,
		type EventForm
	} from '#lib/event-form.js';
	import { describeRepeat, describeWhen, wallClock } from '#lib/occurrence-text.js';
	import { listSyncIssues, onSyncConflict, type SyncIssue } from '#lib/sync-issues.js';
	import { addDays, isoDate, startOfWeek } from '#lib/week-dates.js';

	let now = $state(new Date());

	// Keep the now line current
	$effect(() => {
		const timer = setInterval(() => (now = new Date()), 30_000);
		return () => clearInterval(timer);
	});

	let weekStart = $state(startOfWeek(new Date()));
	const title = $derived(
		new Intl.DateTimeFormat(displayLocale, { month: 'long', year: 'numeric' }).formatRange(
			weekStart,
			addDays(weekStart, 6)
		)
	);

	let calendars = $state<CalendarInfo[]>([]);
	let occurrences = $state<Occurrence[]>([]);
	const calendarsById = $derived(new Map(calendars.map((c) => [c.id, c])));

	// Answers can arrive out of order when the week changes quickly: only the latest counts
	let request = 0;
	// The week `occurrences` belong to
	let loaded: Date | null = null;

	async function load(week: Date) {
		const mine = ++request;
		try {
			const [cals, found] = await Promise.all([
				listCalendars(),
				listOccurrences(isoDate(week), isoDate(addDays(week, 7)))
			]);
			if (mine !== request) return;
			calendars = cals;
			// In the same week, a calendar hidden meanwhile keeps its events in place, faded
			const hidden = new Set(cals.filter((c) => !c.visible).map((c) => c.id));
			occurrences = loaded === week ? keepHidden(occurrences, found, hidden) : found;
			loaded = week;
		} catch (e) {
			// Outside Tauri (`npm run dev` in a browser) there is no backend: show an empty week
			console.error('cannot load the week', e);
		}
	}

	// Shows the change at once; `calendar:changed` then reads the week again
	async function update(id: number, change: CalendarUpdate) {
		const index = calendars.findIndex((c) => c.id === id);
		if (index < 0) return;
		const before = calendars[index];
		calendars[index] = {
			...before,
			name: change.name?.trim() || before.name,
			visible: change.visible ?? before.visible,
			// Going back to the provider's colour shows when Rust answers with it
			color: change.color_override ?? before.color
		};
		try {
			const after = await updateCalendar(id, change);
			const i = calendars.findIndex((c) => c.id === id);
			if (i >= 0) calendars[i] = after;
		} catch (e) {
			console.error('cannot update the calendar', e);
			load(weekStart);
		}
	}

	$effect(() => {
		load(weekStart);
	});

	// After a sync or a removed subscription, read the visible week again (architecture §10)
	$effect(() => {
		const unlisten = onCalendarChanged(() => load(weekStart)).catch(() => undefined);
		return () => {
			unlisten.then((stop) => stop?.());
		};
	});

	// Changes the server did not save (#21). A failed change only shows as `calendar:changed`,
	// so the list is read again after every one.
	let issues = $state<SyncIssue[]>([]);
	let issuesOpen = $state(false);
	let issueRequest = 0;

	async function loadIssues() {
		const mine = ++issueRequest;
		try {
			const found = await listSyncIssues();
			if (mine === issueRequest) issues = found;
		} catch (e) {
			console.error('cannot list the changes not saved', e);
		}
	}

	$effect(() => {
		loadIssues();
		const unlisten = onCalendarChanged(loadIssues).catch(() => undefined);
		return () => {
			unlisten.then((stop) => stop?.());
		};
	});

	/** The titles behind the conflict notice, while it shows */
	let conflicts = $state<string[]>([]);
	const NOTICE_MS = 8000;
	let noticeTimer: ReturnType<typeof setTimeout> | undefined;

	function closeNotice() {
		clearTimeout(noticeTimer);
		conflicts = [];
	}

	$effect(() => {
		const unlisten = onSyncConflict(({ title }) => {
			conflicts = [...conflicts, title];
			clearTimeout(noticeTimer);
			noticeTimer = setTimeout(closeNotice, NOTICE_MS);
			loadIssues();
		}).catch(() => undefined);
		return () => {
			clearTimeout(noticeTimer);
			unlisten.then((stop) => stop?.());
		};
	});

	let selected = $state<{ key: string; anchor: HTMLElement } | null>(null);
	let subscriptionsOpen = $state(false);

	const selectedOccurrence = $derived(
		selected ? occurrences.find((o) => occurrenceKey(o) === selected!.key) : undefined
	);

	// The selected event in full, for its notes and the editor. Read again when a sync or an
	// edit changes the occurrence.
	let selectedEvent = $state<EventDetail | null>(null);
	$effect(() => {
		const occurrence = selectedOccurrence;
		selectedEvent = null;
		if (!occurrence) return;
		let current = true;
		getEvent(occurrence)
			.then((event) => {
				if (current) selectedEvent = event;
			})
			.catch((e) => console.error('cannot read the event', e));
		return () => {
			current = false;
		};
	});

	const details = $derived.by((): EventDetails | null => {
		const occurrence = selectedOccurrence;
		const calendar = occurrence && calendarsById.get(occurrence.calendar_id);
		if (!occurrence || !calendar) return null;
		return {
			title: occurrence.title,
			calendar,
			color: occurrence.color,
			editable: occurrence.editable,
			recurring: selectedEvent ? selectedEvent.repeat !== null : null,
			when: [
				...describeWhen(occurrence),
				...(selectedEvent?.repeat ? [describeRepeat(selectedEvent.repeat)] : [])
			],
			location: occurrence.location ?? undefined,
			notes: selectedEvent?.description ?? undefined
		};
	});

	/** The editor, when open: its starting form, the occurrence it changes, and whether that is
	 * one of a series */
	let editing = $state<{ form: EventForm; target: OccurrenceRef | null; series: boolean } | null>(
		null
	);
	/** The local calendar a save in this editor already created, so a retry does not add another */
	let createdCalendar: number | null = null;

	function newEvent(start: Date, allDay: boolean) {
		close();
		createdCalendar = null;
		const writable = calendars.filter((c) => c.editable);
		// The first visible writable calendar; without one, the editor offers a new local one
		const calendar = writable.find((c) => c.visible) ?? writable[0];
		editing = {
			form: newEventForm({
				calendarId: calendar?.id ?? NEW_LOCAL_CALENDAR,
				start,
				allDay,
				timeZone: displayZone()
			}),
			target: null,
			series: false
		};
	}

	async function editSelected() {
		if (!selectedOccurrence) return;
		const { event_id, original_start } = selectedOccurrence;
		try {
			const target = { event_id, original_start };
			const event = selectedEvent ?? (await getEvent(target));
			editing = {
				form: formFromEvent(event, displayZone()),
				target,
				series: event.repeat !== null
			};
			close();
		} catch (e) {
			console.error('cannot read the event', e);
		}
	}

	async function save(form: EventForm, scope: Scope) {
		const target = editing?.target ?? null;
		let calendarId = form.calendarId;
		// Without a writable calendar the first event makes one on this computer (#15)
		if (calendarId === NEW_LOCAL_CALENDAR) {
			createdCalendar ??= (await createLocalCalendar('Personal')).id;
			calendarId = createdCalendar;
		}
		const draft = draftFromForm({ ...form, calendarId });
		const saved = target ? await updateEvent(target, draft, scope) : await createEvent(draft);
		editing = null;
		// Show the week the event is in; `calendar:changed` reads it. A series that no longer
		// has the occurrence stays on this week.
		if (!saved) return;
		const week = startOfWeek(wallClock(saved.start));
		if (week.getTime() !== weekStart.getTime()) go(week);
	}

	async function removeSelected(scope: Scope) {
		if (!selectedOccurrence) return;
		const { event_id, original_start } = selectedOccurrence;
		await deleteEvent({ event_id, original_start }, scope);
		close();
	}

	// Hiding the calendar of the open event, or the event going away in a sync, closes the
	// popover
	$effect(() => {
		if (selected && (!details || !details.calendar.visible)) selected = null;
	});

	function close() {
		selected = null;
	}

	function go(week: Date) {
		close();
		weekStart = week;
	}
</script>

<div class="app">
	<Sidebar
		today={now}
		{weekStart}
		{calendars}
		onupdate={update}
		onsubscriptions={() => (subscriptionsOpen = true)}
		issueCount={issues.length}
		onissues={() => (issuesOpen = true)}
		oncreatecalendar={async (name) => {
			await createLocalCalendar(name);
		}}
		onremovecalendar={removeCalendar}
	/>
	<main>
		<Toolbar
			{title}
			onprevious={() => go(addDays(weekStart, -7))}
			ontoday={() => go(startOfWeek(new Date()))}
			onnext={() => go(addDays(weekStart, 7))}
			onnew={() => newEvent(defaultStart(weekStart, new Date()), false)}
		/>
		<WeekView
			{weekStart}
			{now}
			calendars={calendarsById}
			{occurrences}
			selectedKey={selected?.key ?? null}
			onselect={(key, anchor) => (selected = { key, anchor })}
			onscroll={close}
			oncreate={newEvent}
		/>
	</main>
</div>

<SubscriptionsDialog bind:open={subscriptionsOpen} />
<SyncIssuesDialog bind:open={issuesOpen} {issues} onchanged={loadIssues} />

{#if conflicts.length > 0}
	<SyncNotice
		titles={conflicts}
		onshow={() => {
			closeNotice();
			issuesOpen = true;
		}}
		onclose={closeNotice}
	/>
{/if}

{#if selected && details}
	{#key selected.key}
		<EventPopover
			{details}
			anchor={selected.anchor}
			onclose={close}
			onedit={editSelected}
			ondelete={removeSelected}
		/>
	{/key}
{/if}

{#if editing}
	<EventEditor
		initial={editing.form}
		isNew={editing.target === null}
		series={editing.series}
		{calendars}
		displayZone={displayZone()}
		onsave={save}
		onclose={() => (editing = null)}
	/>
{/if}

<style>
	.app {
		display: flex;
		height: 100vh;
	}

	main {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}
</style>
