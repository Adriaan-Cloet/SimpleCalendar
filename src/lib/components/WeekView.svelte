<script lang="ts">
	import { calendarColor } from '#lib/calendar-color.js';
	import type { AllDayEvent, Calendar, TimedEvent } from '#lib/prototype/fake-week.js';
	import {
		addDays,
		atMinutes,
		isSameDay,
		minutesSinceMidnight
	} from '#lib/prototype/week-dates.js';
	import { layoutDay } from '#lib/overlap-layout.js';
	import EventBlock from './EventBlock.svelte';

	interface Props {
		weekStart: Date;
		now: Date;
		calendars: Record<string, Calendar>;
		timedEvents: TimedEvent[];
		allDayEvents: AllDayEvent[];
		visible: Record<string, boolean>;
		selectedId: string | null;
		onselect: (id: string, anchor: HTMLElement) => void;
		onscroll: () => void;
	}

	let {
		weekStart,
		now,
		calendars,
		timedEvents,
		allDayEvents,
		visible,
		selectedId,
		onselect,
		onscroll
	}: Props = $props();

	const days = $derived(Array.from({ length: 7 }, (_, i) => addDays(weekStart, i)));
	const todayIndex = $derived(days.findIndex((day) => isSameDay(day, now)));
	const nowMinutes = $derived(minutesSinceMidnight(now));
	const hours = Array.from({ length: 23 }, (_, i) => i + 1);
	const allDayRows = $derived(Math.max(1, ...allDayEvents.map((e) => e.row + 1)));

	// Each day's timed events with their place, in drawing order. Hidden calendars stay in the
	// layout, so hiding one fades its events without moving the others (design.md §8).
	const eventsById = $derived(new Map(timedEvents.map((e) => [e.id, e])));
	const layouts = $derived(
		days.map((_, i) =>
			layoutDay(timedEvents.filter((e) => e.day === i)).map((placement) => ({
				placement,
				event: eventsById.get(placement.id)!
			}))
		)
	);

	const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: 'short' });
	const dayLabel = new Intl.DateTimeFormat(undefined, { dateStyle: 'full' });
	const timeFormat = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
	// The gutter always uses a 24-hour clock ("08:00"), whatever the locale
	const gutterFormat = new Intl.DateTimeFormat(undefined, {
		hour: '2-digit',
		minute: '2-digit',
		hourCycle: 'h23'
	});

	function isWeekend(day: Date): boolean {
		return day.getDay() === 0 || day.getDay() === 6;
	}

	// An hour label too close to the now label would overlap it
	function hourHidden(hour: number): boolean {
		return todayIndex >= 0 && Math.abs(hour * 60 - nowMinutes) < 12;
	}

	let scroller: HTMLElement | undefined = $state();
	let body: HTMLElement | undefined = $state();

	// Open at 07:30 rather than midnight
	$effect(() => {
		if (!scroller || !body) return;
		scroller.scrollTop = (body.offsetHeight / 24) * 7.5;
	});
</script>

<div class="week" bind:this={scroller} {onscroll}>
	<div class="sticky">
		<div class="row header">
			<div></div>
			{#each days as day, i (i)}
				<div
					class={{ 'day-header': true, today: i === todayIndex }}
					aria-label={dayLabel.format(day)}
				>
					<span class="weekday">{weekdayFormat.format(day)}</span>
					<span class="date tabular">{day.getDate()}</span>
				</div>
			{/each}
		</div>

		<div class="row all-day" style:--rows={allDayRows}>
			<div class="gutter-label">all-day</div>
			{#each days as day, i (i)}
				<div
					class={{ 'all-day-cell': true, weekend: isWeekend(day) }}
					style:grid-column={i + 2}
				></div>
			{/each}
			{#each allDayEvents as event (event.id)}
				{@const calendar = calendars[event.calendarId]}
				<button
					type="button"
					class={{
						'all-day-event': true,
						selected: event.id === selectedId,
						hidden: !visible[event.calendarId]
					}}
					style:grid-column="{event.firstDay + 2} / {event.lastDay + 3}"
					style:grid-row={event.row + 1}
					style:--cal={calendarColor(calendar.color)}
					data-event
					aria-pressed={event.id === selectedId}
					aria-hidden={!visible[event.calendarId]}
					tabindex={visible[event.calendarId] ? undefined : -1}
					onclick={(e) => onselect(event.id, e.currentTarget)}>{event.title}</button
				>
			{/each}
		</div>
	</div>

	<div class="row body" bind:this={body}>
		<div class="gutter tabular">
			{#each hours as hour (hour)}
				<span class="hour-label" style:--hour={hour} class:hidden={hourHidden(hour)}>
					{gutterFormat.format(atMinutes(weekStart, hour * 60))}
				</span>
			{/each}
			{#if todayIndex >= 0}
				<span class="hour-label now-label" style:--hour={nowMinutes / 60}>
					{gutterFormat.format(now)}
				</span>
			{/if}
		</div>

		<div class="hour-lines" aria-hidden="true">
			{#each hours as hour (hour)}
				<div class="hour-line" style:--hour={hour}></div>
			{/each}
		</div>

		{#each days as day, i (i)}
			<div class={{ day: true, weekend: isWeekend(day) }} style:grid-column={i + 2}>
				{#each layouts[i] as { event, placement } (event.id)}
					<EventBlock
						{event}
						{placement}
						calendar={calendars[event.calendarId]}
						timeLabel={timeFormat.format(atMinutes(day, event.start))}
						hidden={!visible[event.calendarId]}
						selected={event.id === selectedId}
						onselect={(anchor) => onselect(event.id, anchor)}
					/>
				{/each}
				{#if i === todayIndex}
					<div class="now-line" style:--minutes={nowMinutes} aria-hidden="true"></div>
				{/if}
			</div>
		{/each}
	</div>
</div>

<style>
	.week {
		flex: 1;
		min-height: 0;
		overflow-y: auto;
		/* Contain the absolutely positioned events and the sticky header */
		position: relative;
	}

	.row {
		display: grid;
		grid-template-columns: var(--gutter-width) repeat(7, minmax(0, 1fr));
	}

	/* Header and all-day row live inside the scroller, so they always line up with the grid
	   no matter how wide the scrollbar is in either webview. */
	.sticky {
		position: sticky;
		top: 0;
		z-index: 4;
		background: var(--surface);
		border-bottom: 1px solid var(--separator);
	}

	/* Day headers */
	.day-header {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-xs);
		height: 36px;
		padding: 0 var(--space-xs);
	}

	.weekday {
		color: var(--text-secondary);
	}

	.date {
		display: grid;
		place-items: center;
		min-width: 24px;
		height: 24px;
		padding: 0 var(--space-xs);
		border-radius: var(--radius-full);
		font-weight: var(--weight-semibold);
	}

	.today .weekday {
		color: var(--now-text);
	}

	.today .date {
		color: var(--on-now);
		background: var(--now);
	}

	/* All-day row */
	.all-day {
		grid-template-rows: repeat(var(--rows), 22px);
		row-gap: var(--space-2xs);
		padding: var(--space-2xs) 0;
	}

	.gutter-label {
		grid-column: 1;
		grid-row: 1 / -1;
		align-self: center;
		padding-right: var(--space-sm);
		text-align: right;
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
		color: var(--text-secondary);
	}

	.all-day-cell {
		grid-row: 1 / -1;
		margin: calc(-1 * var(--space-2xs)) 0;
		border-left: 1px solid var(--grid-line);
	}

	.all-day-cell.weekend {
		background: var(--surface-weekend);
	}

	.all-day-event {
		z-index: 1;
		min-width: 0;
		margin: 0 2px 0 1px;
		padding: 0 var(--space-sm);
		border: 0;
		border-radius: var(--radius-sm);
		text-align: left;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
		color: color-mix(in oklab, var(--cal) var(--event-text-mix), var(--text));
		background: color-mix(in oklab, var(--cal) var(--event-fill-mix), var(--surface));
		transition:
			opacity var(--duration-base) var(--ease-standard),
			visibility 0s linear;
	}

	.all-day-event:hover {
		background: color-mix(in oklab, var(--cal) var(--event-hover-mix), var(--surface));
	}

	.all-day-event.selected {
		background: color-mix(in oklab, var(--cal) var(--event-selected-mix), var(--surface));
		box-shadow: 0 0 0 1px var(--cal);
	}

	.all-day-event.hidden {
		opacity: 0;
		visibility: hidden;
		pointer-events: none;
		transition:
			opacity var(--duration-base) var(--ease-exit),
			visibility 0s linear var(--duration-base);
	}

	/* Time grid */
	.body {
		position: relative;
		height: calc(24 * var(--hour-height));
	}

	.gutter {
		position: relative;
		grid-column: 1;
	}

	.hour-label {
		position: absolute;
		right: var(--space-sm);
		top: calc(var(--hour) * var(--hour-height));
		transform: translateY(-50%);
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
		color: var(--text-secondary);
		white-space: nowrap;
	}

	.hour-label.hidden {
		visibility: hidden;
	}

	.now-label {
		color: var(--now-text);
		font-weight: var(--weight-semibold);
	}

	.hour-lines {
		position: absolute;
		inset: 0 0 0 var(--gutter-width);
		z-index: 1;
		pointer-events: none;
	}

	.hour-line {
		position: absolute;
		left: 0;
		right: 0;
		top: calc(var(--hour) * var(--hour-height));
		border-top: 1px solid var(--grid-line);
	}

	.day {
		/* What the channel between overlapping events is drawn in (EventBlock) */
		--day-surface: var(--surface);
		position: relative;
		border-left: 1px solid var(--grid-line);
	}

	.day.weekend {
		--day-surface: var(--surface-weekend);
		background: var(--surface-weekend);
	}

	/* Events sit above the hour lines */
	.day :global(.event) {
		z-index: 2;
	}

	.day :global(.event.selected) {
		z-index: 3;
	}

	.now-line {
		position: absolute;
		left: 0;
		right: 0;
		top: calc(var(--hour-height) * var(--minutes) / 60 - 1px);
		z-index: 3;
		height: 2px;
		background: var(--now);
		pointer-events: none;
	}

	.now-line::before {
		content: '';
		position: absolute;
		left: -5px;
		top: -4px;
		width: 10px;
		height: 10px;
		border-radius: var(--radius-full);
		background: var(--now);
	}
</style>
