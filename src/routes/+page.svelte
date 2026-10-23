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
	import MonthGrid from '#lib/components/MonthGrid.svelte';
	import SyncNotice from '#lib/components/SyncNotice.svelte';
	import Toolbar from '#lib/components/Toolbar.svelte';
	import TimeGrid from '#lib/components/TimeGrid.svelte';
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
	import { listSources, onSyncStatus, syncNow, type SyncState } from '#lib/sources.js';
	import { listSyncIssues, onSyncConflict, type SyncIssue } from '#lib/sync-issues.js';
	import { summarize } from '#lib/sync-summary.js';
	import {
		inRange,
		ownRange,
		step,
		viewRange,
		viewTitle,
		type View,
		type ViewRange
	} from '#lib/view-range.js';
	import { addDays, isoDate } from '#lib/week-dates.js';

	let now = $state(new Date());

	// Keep the now line current
	$effect(() => {
		const timer = setInterval(() => (now = new Date()), 30_000);
		return () => clearInterval(timer);
	});

	// The selected date and the view around it (#23, #24). Every view keeps the date, so
	// switching shows the same day in its week or month and back.
	let view = $state<View>('week');
	let date = $state(new Date());
	const range = $derived(viewRange(view, date));
	const title = $derived(viewTitle(view, date, displayLocale));

	// A day opened from the month view offers a way back to the month, of the day now on screen,
	// until another view is chosen (#24)
	let fromMonth = $state(false);
	const monthName = new Intl.DateTimeFormat(displayLocale, { month: 'long' });
	const back = $derived(
		fromMonth && view === 'day'
			? { label: monthName.format(date), onback: () => go(date, 'month') }
			: undefined
	);

	let calendars = $state<CalendarInfo[]>([]);
	let occurrences = $state<Occurrence[]>([]);
	const calendarsById = $derived(new Map(calendars.map((c) => [c.id, c])));

	// Answers can arrive out of order when the days change quickly: only the latest counts
	let request = 0;
	// The days `occurrences` belong to, as `start/days`
	let loaded: string | null = null;

	async function load({ start, days }: ViewRange) {
		const mine = ++request;
		const key = `${isoDate(start)}/${days}`;
		try {
			const [cals, found] = await Promise.all([
				listCalendars(),
				listOccurrences(isoDate(start), isoDate(addDays(start, days)))
			]);
			if (mine !== request) return;
			calendars = cals;
			// On the same days, a calendar hidden meanwhile keeps its events in place, faded
			const hidden = new Set(cals.filter((c) => !c.visible).map((c) => c.id));
			occurrences = loaded === key ? keepHidden(occurrences, found, hidden) : found;
			loaded = key;
		} catch (e) {
			// Outside Tauri (`npm run dev` in a browser) there is no backend: show an empty week
			console.error('cannot load the days', e);
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
			load(range);
		}
	}

	$effect(() => {
		load(range);
	});

	// After a sync or a removed subscription, read the visible days again (architecture §10)
	$effect(() => {
		const unlisten = onCalendarChanged(() => load(range)).catch(() => undefined);
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

	// What each source's sync is doing, for the toolbar's indicator (#22): the stored outcome
	// of its last cycle, then `sync:status`. Read again after a change to the sources, which
	// `calendar:changed` and closing the Accounts dialog stand for.
	let syncStates = $state<Record<string, SyncState>>({});
	const syncSummary = $derived(summarize(syncStates));
	let sourcesRequest = 0;

	async function loadSources() {
		const mine = ++sourcesRequest;
		try {
			const sources = await listSources();
			if (mine !== sourcesRequest) return;
			// A cycle that started meanwhile keeps showing as syncing
			syncStates = Object.fromEntries(
				sources.map((s) => [s.id, syncStates[s.id] === 'syncing' ? 'syncing' : s.state])
			);
		} catch (e) {
			console.error('cannot list the accounts', e);
		}
	}

	$effect(() => {
		loadSources();
		const unlistenStatus = onSyncStatus(({ source_id, state }) => {
			syncStates[source_id] = state;
		}).catch(() => undefined);
		const unlistenChanged = onCalendarChanged(loadSources).catch(() => undefined);
		return () => {
			unlistenStatus.then((stop) => stop?.());
			unlistenChanged.then((stop) => stop?.());
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
		// Show the days the event is on; `calendar:changed` reads them. A series that no longer
		// has the occurrence stays where it is.
		if (!saved) return;
		const start = wallClock(saved.start);
		if (!inRange(range, start)) go(start);
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

	/** Shows `to`, in `next` view or the current one */
	function go(to: Date, next: View = view) {
		close();
		date = to;
		view = next;
		if (next !== 'day') fromMonth = false;
	}

	// Escape on a day opened from the month goes back, unless it closes something first. This
	// listener runs before the popover's, which is added when the popover opens.
	function onkeydown(e: KeyboardEvent) {
		if (e.key !== 'Escape' || e.defaultPrevented || !back) return;
		if (selected || editing || subscriptionsOpen || issuesOpen) return;
		back.onback();
	}
</script>

<svelte:window {onkeydown} />

<div class="app">
	<Sidebar
		today={now}
		{view}
		{date}
		onday={(day) => {
			// The mini month opens the day itself, so there is no month to go back to
			fromMonth = false;
			go(day, 'day');
		}}
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
			{view}
			{back}
			onview={(next) => {
				fromMonth = false;
				go(date, next);
			}}
			onprevious={() => go(step(view, date, -1))}
			ontoday={() => go(new Date())}
			onnext={() => go(step(view, date, 1))}
			onnew={() => newEvent(defaultStart(ownRange(view, date), new Date()), false)}
			sync={syncSummary}
			onsync={() => syncNow().catch((e) => console.error('cannot sync', e))}
			onaccounts={() => (subscriptionsOpen = true)}
		/>
		{#if view === 'month'}
			<MonthGrid
				start={range.start}
				dayCount={range.days}
				month={date}
				{now}
				calendars={calendarsById}
				{occurrences}
				selectedKey={selected?.key ?? null}
				onselect={(key, anchor) => (selected = { key, anchor })}
				onday={(day) => {
					go(day, 'day');
					fromMonth = true;
				}}
			/>
		{:else}
			<TimeGrid
				start={range.start}
				dayCount={range.days}
				{now}
				calendars={calendarsById}
				{occurrences}
				selectedKey={selected?.key ?? null}
				onselect={(key, anchor) => (selected = { key, anchor })}
				onscroll={close}
				oncreate={newEvent}
				onday={view === 'week' ? (day) => go(day, 'day') : undefined}
			/>
		{/if}
	</main>
</div>

<SubscriptionsDialog bind:open={subscriptionsOpen} onclosed={loadSources} />
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
