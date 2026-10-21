<script lang="ts">
	import {
		listCalendars,
		listOccurrences,
		occurrenceKey,
		onCalendarChanged,
		updateCalendar,
		type CalendarUpdate,
		type CalendarInfo,
		type Occurrence
	} from '#lib/calendar.js';
	import EventPopover, { type EventDetails } from '#lib/components/EventPopover.svelte';
	import Sidebar from '#lib/components/Sidebar.svelte';
	import SubscriptionsDialog from '#lib/components/SubscriptionsDialog.svelte';
	import Toolbar from '#lib/components/Toolbar.svelte';
	import WeekView from '#lib/components/WeekView.svelte';
	import { keepHidden } from '#lib/keep-hidden.js';
	import { describeWhen } from '#lib/occurrence-text.js';
	import { addDays, isoDate, startOfWeek } from '#lib/week-dates.js';

	let now = $state(new Date());

	// Keep the now line current
	$effect(() => {
		const timer = setInterval(() => (now = new Date()), 30_000);
		return () => clearInterval(timer);
	});

	let weekStart = $state(startOfWeek(new Date()));
	const title = $derived(
		new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).formatRange(
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

	let selected = $state<{ key: string; anchor: HTMLElement } | null>(null);
	let subscriptionsOpen = $state(false);

	const details = $derived.by((): EventDetails | null => {
		if (!selected) return null;
		const key = selected.key;
		const occurrence = occurrences.find((o) => occurrenceKey(o) === key);
		const calendar = occurrence && calendarsById.get(occurrence.calendar_id);
		if (!occurrence || !calendar) return null;
		return {
			title: occurrence.title,
			calendar,
			color: occurrence.color,
			editable: occurrence.editable,
			when: describeWhen(occurrence),
			location: occurrence.location ?? undefined
		};
	});

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
	/>
	<main>
		<Toolbar
			{title}
			onprevious={() => go(addDays(weekStart, -7))}
			ontoday={() => go(startOfWeek(new Date()))}
			onnext={() => go(addDays(weekStart, 7))}
		/>
		<WeekView
			{weekStart}
			{now}
			calendars={calendarsById}
			{occurrences}
			selectedKey={selected?.key ?? null}
			onselect={(key, anchor) => (selected = { key, anchor })}
			onscroll={close}
		/>
	</main>
</div>

<SubscriptionsDialog bind:open={subscriptionsOpen} />

{#if selected && details}
	{#key selected.key}
		<EventPopover {details} anchor={selected.anchor} onclose={close} />
	{/key}
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
