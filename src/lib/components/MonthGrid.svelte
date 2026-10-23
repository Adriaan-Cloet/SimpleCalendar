<script lang="ts">
	// The month view (issue #24): a cell per day of the weeks that cover the month, each listing
	// its events as quiet lines, with "+N more" when they do not fit. A click on an event opens
	// it; a click anywhere else in a cell opens that day.
	import { occurrenceKey, type CalendarInfo, type Occurrence } from '#lib/calendar.js';
	import { calendarColor } from '#lib/calendar-color.js';
	import { displayLocale } from '#lib/display-format.js';
	import { fitDay, placeMonth } from '#lib/month-layout.js';
	import { wallClock } from '#lib/occurrence-text.js';
	import { addDays, isSameDay, isoDate } from '#lib/week-dates.js';

	interface Props {
		/** The first day shown: the Monday of the month's first week */
		start: Date;
		/** Days shown, whole weeks */
		dayCount: number;
		/** A day of the month on screen; days outside it are faded */
		month: Date;
		now: Date;
		calendars: Map<number, CalendarInfo>;
		/** The occurrences of those days from `list_occurrences`, sorted by start */
		occurrences: Occurrence[];
		selectedKey: string | null;
		onselect: (key: string, anchor: HTMLElement) => void;
		/** A click on a day outside its events: open that day */
		onday: (day: Date) => void;
	}

	let { start, dayCount, month, now, calendars, occurrences, selectedKey, onselect, onday }: Props =
		$props();

	/** Height of one line, and the gap between lines, in px: the same as in the styles below */
	const LINE = 18;
	const GAP = 2;

	const days = $derived(Array.from({ length: dayCount }, (_, i) => addDays(start, i)));
	const weekdays = $derived(days.slice(0, 7));

	// Hidden calendars leave the month: a day's lines simply close up
	const visible = $derived(occurrences.filter((o) => calendars.get(o.calendar_id)?.visible));
	const lists = $derived(placeMonth(visible, days.map(isoDate)));

	// Every cell has the same height, so measuring the first one's list is enough
	let listHeight = $state(0);
	const capacity = $derived(Math.floor((listHeight + GAP) / (LINE + GAP)));

	const weekdayFormat = new Intl.DateTimeFormat(displayLocale, { weekday: 'short' });
	const dayLabel = new Intl.DateTimeFormat(displayLocale, { dateStyle: 'full' });
	const monthShort = new Intl.DateTimeFormat(displayLocale, { month: 'short' });
	const timeFormat = new Intl.DateTimeFormat(displayLocale, { hour: '2-digit', minute: '2-digit' });

	function isWeekend(day: Date): boolean {
		return day.getDay() === 0 || day.getDay() === 6;
	}

	// A click on an event selects it; anywhere else in the cell opens the day
	function openDay(e: MouseEvent, day: Date) {
		if ((e.target as Element).closest('[data-event]')) return;
		onday(day);
	}
</script>

<div class="month" style:--weeks={dayCount / 7}>
	<div class="weekdays" aria-hidden="true">
		{#each weekdays as day (day.getTime())}
			<span class={{ weekday: true, weekend: isWeekend(day) }}>{weekdayFormat.format(day)}</span>
		{/each}
	</div>

	<div class="cells">
		{#each days as day, i (day.getTime())}
			{@const items = lists[i]}
			{@const fit = fitDay(items.length, capacity)}
			{@const today = isSameDay(day, now)}
			<!-- The date button and "+N more" are the keyboard way to open the day -->
			<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
			<div
				class={{
					cell: true,
					weekend: isWeekend(day),
					outside: day.getMonth() !== month.getMonth()
				}}
				onclick={(e) => openDay(e, day)}
			>
				<button
					type="button"
					class={{ date: true, today, tabular: true }}
					aria-label="{dayLabel.format(day)}, open as a day"
					title="Show this day"
				>
					{#if day.getDate() === 1}{monthShort.format(day)}&nbsp;{/if}<span class="number"
						>{day.getDate()}</span
					>
				</button>

				{#if i === 0}
					<div class="list" bind:clientHeight={listHeight}>
						{@render lines(items, fit)}
					</div>
				{:else}
					<div class="list">
						{@render lines(items, fit)}
					</div>
				{/if}
			</div>
		{/each}
	</div>
</div>

{#snippet lines(items: ReturnType<typeof placeMonth>[number], fit: ReturnType<typeof fitDay>)}
	{#each items.slice(0, fit.shown) as { occurrence, allDay } (occurrenceKey(occurrence))}
		{@const key = occurrenceKey(occurrence)}
		<button
			type="button"
			class={{ line: true, 'all-day': allDay, selected: key === selectedKey }}
			style:--cal={calendarColor(occurrence.color, occurrence.calendar_id)}
			data-event
			aria-pressed={key === selectedKey}
			onclick={(e) => onselect(key, e.currentTarget)}
		>
			{#if !allDay}
				<span class="dot" aria-hidden="true"></span>
				<span class="time tabular">{timeFormat.format(wallClock(occurrence.start))}</span>
			{/if}
			<span class="title">{occurrence.title}</span>
		</button>
	{/each}
	{#if fit.more > 0}
		<button type="button" class="more" title="Show this day">+{fit.more} more</button>
	{/if}
{/snippet}

<style>
	.month {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-height: 0;
	}

	.weekdays,
	.cells {
		display: grid;
		grid-template-columns: repeat(7, minmax(0, 1fr));
	}

	.weekdays {
		border-bottom: 1px solid var(--separator);
	}

	.weekday {
		padding: var(--space-sm) var(--space-sm) var(--space-xs);
		text-align: right;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.cells {
		flex: 1;
		min-height: 0;
		grid-template-rows: repeat(var(--weeks), minmax(0, 1fr));
	}

	.cell {
		display: flex;
		flex-direction: column;
		min-height: 0;
		padding: var(--space-xs) var(--space-xs) var(--space-sm);
		border-left: 1px solid var(--grid-line);
		border-top: 1px solid var(--grid-line);
		cursor: pointer;
	}

	.cell:nth-child(7n + 1) {
		border-left: 0;
	}

	.cell:nth-child(-n + 7) {
		border-top: 0;
	}

	.cell.weekend {
		background: var(--surface-weekend);
	}

	/* The day number, top right as in the column header above it */
	.date {
		align-self: flex-end;
		display: inline-flex;
		align-items: center;
		height: 24px;
		margin-bottom: var(--space-2xs);
		padding: 0;
		border: 0;
		background: transparent;
		color: inherit;
		font: inherit;
		font-weight: var(--weight-semibold);
		white-space: nowrap;
	}

	.number {
		display: grid;
		place-items: center;
		min-width: 24px;
		height: 24px;
		padding: 0 var(--space-xs);
		border-radius: var(--radius-full);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.cell:hover .number {
		background: var(--fill-hover);
	}

	.outside .date {
		color: var(--text-tertiary);
	}

	.date.today .number,
	.cell:hover .date.today .number {
		color: var(--on-now);
		background: var(--now);
	}

	.list {
		display: flex;
		flex-direction: column;
		gap: var(--space-2xs);
		flex: 1;
		min-height: 0;
		overflow: hidden;
	}

	/* One quiet line per event: a dot, the start time, the title */
	.line {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		flex: none;
		height: 18px;
		min-width: 0;
		padding: 0 var(--space-xs);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text);
		font: inherit;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		text-align: left;
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.line:hover {
		background: var(--fill-hover);
	}

	.line.selected {
		background: color-mix(in oklab, var(--cal) var(--event-fill-mix), var(--surface));
		box-shadow: 0 0 0 1px var(--cal);
	}

	.dot {
		flex: none;
		width: 7px;
		height: 7px;
		border-radius: var(--radius-full);
		background: var(--cal);
	}

	.time {
		flex: none;
		font-size: var(--text-caption);
		color: var(--text-secondary);
	}

	.title {
		min-width: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}

	/* All-day: a soft tint in the calendar's colour, as in the week's all-day row */
	.line.all-day {
		font-weight: var(--weight-semibold);
		color: color-mix(in oklab, var(--cal) var(--event-text-mix), var(--text));
		background: color-mix(in oklab, var(--cal) var(--event-fill-mix), var(--surface));
	}

	.line.all-day:hover {
		background: color-mix(in oklab, var(--cal) var(--event-hover-mix), var(--surface));
	}

	.line.all-day.selected {
		background: color-mix(in oklab, var(--cal) var(--event-selected-mix), var(--surface));
	}

	.more {
		flex: none;
		align-self: flex-start;
		height: 18px;
		padding: 0 var(--space-xs);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font: inherit;
		font-size: var(--text-caption);
		font-weight: var(--weight-medium);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.more:hover {
		background: var(--fill-hover);
	}
</style>
