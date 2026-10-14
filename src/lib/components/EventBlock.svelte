<script lang="ts">
	import { calendarColor } from '#lib/calendar-color.js';
	import type { Calendar, TimedEvent } from '#lib/prototype/fake-week.js';

	interface Props {
		event: TimedEvent;
		calendar: Calendar;
		timeLabel: string;
		hidden: boolean;
		selected: boolean;
		onselect: (anchor: HTMLElement) => void;
	}

	let { event, calendar, timeLabel, hidden, selected, onselect }: Props = $props();

	const duration = $derived(event.end - event.start);
	// Under 45 minutes there is room for one line only: title and start time side by side
	const compact = $derived(duration < 45);
</script>

<button
	type="button"
	class={{ event: true, compact, tiny: duration <= 15, selected, hidden }}
	style:--cal={calendarColor(calendar.color)}
	style:--start={event.start}
	style:--duration={duration}
	style:--column={event.column}
	style:--columns={event.columns}
	data-event
	aria-pressed={selected}
	aria-hidden={hidden}
	tabindex={hidden ? -1 : undefined}
	onclick={(e) => onselect(e.currentTarget)}
>
	<span class="title">{event.title}</span>
	<span class="time tabular">{timeLabel}</span>
	{#if !compact && event.location}
		<span class="location">{event.location}</span>
	{/if}
</button>

<style>
	.event {
		position: absolute;
		top: calc(var(--hour-height) * var(--start) / 60);
		/* 1px clear of the day's left border, 4px free on the right to reach the empty slot */
		left: calc(1px + (100% - var(--space-xs) - 1px) * var(--column) / var(--columns));
		width: calc((100% - var(--space-xs) - 1px) / var(--columns) - 1px);
		/* 1px gap to an event that starts right after this one */
		height: calc(var(--hour-height) * var(--duration) / 60 - 1px);
		display: flex;
		flex-direction: column;
		align-items: stretch;
		gap: 0;
		overflow: hidden;
		/* Text starts 8px in, like all-day events; ellipsis gets 4px on the right */
		padding: var(--space-2xs) var(--space-xs) var(--space-2xs) var(--space-sm);
		border: 0;
		border-radius: var(--radius-sm);
		text-align: left;
		color: color-mix(in oklab, var(--cal) var(--event-text-mix), var(--text));
		background: color-mix(in oklab, var(--cal) var(--event-fill-mix), var(--surface));
		transition:
			opacity var(--duration-base) var(--ease-standard),
			visibility 0s linear;
	}

	.event:hover {
		background: color-mix(in oklab, var(--cal) var(--event-hover-mix), var(--surface));
	}

	.event.selected {
		background: color-mix(in oklab, var(--cal) var(--event-selected-mix), var(--surface));
		box-shadow: 0 0 0 1px var(--cal);
	}

	.event.hidden {
		opacity: 0;
		visibility: hidden;
		pointer-events: none;
		/* Fade first, then hide, so the fade is visible */
		transition:
			opacity var(--duration-base) var(--ease-exit),
			visibility 0s linear var(--duration-base);
	}

	.event.compact {
		flex-direction: row;
		align-items: baseline;
		gap: var(--space-xs);
	}

	.event.tiny {
		padding-top: 0;
		padding-bottom: 0;
	}

	.title,
	.time,
	.location {
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.title {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
	}

	.time,
	.location {
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
	}

	.compact .title {
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
	}

	/* In one line the title wins: it keeps its width and the time gets what is left */
	.compact .title {
		flex: none;
		max-width: 100%;
	}
</style>
