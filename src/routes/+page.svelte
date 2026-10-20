<script lang="ts">
	// Design prototype: a static week with fake data. See docs/design.md.
	import EventPopover, { type EventDetails } from '#lib/components/EventPopover.svelte';
	import Sidebar from '#lib/components/Sidebar.svelte';
	import SubscriptionsDialog from '#lib/components/SubscriptionsDialog.svelte';
	import Toolbar from '#lib/components/Toolbar.svelte';
	import WeekView from '#lib/components/WeekView.svelte';
	import { allDayEvents, calendars, timedEvents } from '#lib/prototype/fake-week.js';
	import { addDays, atMinutes, startOfWeek } from '#lib/prototype/week-dates.js';

	let now = $state(new Date());

	// Keep the now line current
	$effect(() => {
		const timer = setInterval(() => (now = new Date()), 30_000);
		return () => clearInterval(timer);
	});

	const weekStart = $derived(startOfWeek(now));
	const title = $derived(
		new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).formatRange(
			weekStart,
			addDays(weekStart, 6)
		)
	);
	const calendarsById = Object.fromEntries(calendars.map((c) => [c.id, c]));

	let visible = $state<Record<string, boolean>>(
		Object.fromEntries(calendars.map((c) => [c.id, true]))
	);
	let selected = $state<{ id: string; anchor: HTMLElement } | null>(null);
	let subscriptionsOpen = $state(false);

	const dateFormat = new Intl.DateTimeFormat(undefined, {
		weekday: 'long',
		day: 'numeric',
		month: 'long'
	});
	const timeFormat = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });

	const details = $derived.by((): EventDetails | null => {
		if (!selected) return null;
		const id = selected.id;
		const timed = timedEvents.find((e) => e.id === id);
		if (timed) {
			const day = addDays(weekStart, timed.day);
			return {
				title: timed.title,
				calendar: calendarsById[timed.calendarId],
				when: [
					dateFormat.format(day),
					timeFormat.formatRange(atMinutes(day, timed.start), atMinutes(day, timed.end))
				],
				location: timed.location,
				notes: timed.notes
			};
		}
		const allDay = allDayEvents.find((e) => e.id === id);
		if (!allDay) return null;
		return {
			title: allDay.title,
			calendar: calendarsById[allDay.calendarId],
			when: [
				dateFormat.formatRange(
					addDays(weekStart, allDay.firstDay),
					addDays(weekStart, allDay.lastDay)
				),
				'All day'
			]
		};
	});

	// Hiding the calendar of the open event closes the popover
	$effect(() => {
		if (details && !visible[details.calendar.id]) selected = null;
	});

	function close() {
		selected = null;
	}
</script>

<div class="app">
	<Sidebar
		today={now}
		{calendars}
		bind:visible
		onsubscriptions={() => (subscriptionsOpen = true)}
	/>
	<main>
		<Toolbar {title} />
		<WeekView
			{weekStart}
			{now}
			calendars={calendarsById}
			{timedEvents}
			{allDayEvents}
			{visible}
			selectedId={selected?.id ?? null}
			onselect={(id, anchor) => (selected = { id, anchor })}
			onscroll={close}
		/>
	</main>
</div>

<SubscriptionsDialog bind:open={subscriptionsOpen} />

{#if selected && details}
	{#key selected.id}
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
