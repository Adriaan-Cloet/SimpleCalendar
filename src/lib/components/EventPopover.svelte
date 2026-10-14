<script lang="ts" module>
	import type { Calendar } from '#lib/prototype/fake-week.js';

	export interface EventDetails {
		title: string;
		calendar: Calendar;
		/** One line per entry, e.g. the date and the time range */
		when: string[];
		location?: string;
		notes?: string;
	}
</script>

<script lang="ts">
	import { calendarColor } from '#lib/calendar-color.js';

	interface Props {
		details: EventDetails;
		anchor: HTMLElement;
		onclose: () => void;
	}

	let { details, anchor, onclose }: Props = $props();

	let popover: HTMLElement | undefined = $state();
	let position = $state({ left: 0, top: 0, side: 'right' as 'left' | 'right', ready: false });

	const MARGIN = 8;

	// Place the popover beside the event: right if it fits, otherwise left
	$effect(() => {
		if (!popover) return;
		const rect = anchor.getBoundingClientRect();
		const width = popover.offsetWidth;
		const height = popover.offsetHeight;
		let left = rect.right + MARGIN;
		let side: 'left' | 'right' = 'right';
		if (left + width > window.innerWidth - MARGIN) {
			left = Math.max(MARGIN, rect.left - MARGIN - width);
			side = 'left';
		}
		const top = Math.min(
			Math.max(MARGIN, rect.top),
			Math.max(MARGIN, window.innerHeight - height - MARGIN)
		);
		position = { left, top, side, ready: true };
	});

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			onclose();
			anchor.focus();
		}
	}

	function onpointerdown(e: PointerEvent) {
		const target = e.target as Element;
		// A click on another event selects that event instead; the page handles it
		if (popover?.contains(target) || target.closest('[data-event]')) return;
		onclose();
	}

	const readOnlyReason = $derived(
		details.calendar.source === 'ics'
			? 'Subscribed calendars can’t be edited.'
			: 'You can view this calendar but not change it.'
	);
</script>

<svelte:window {onkeydown} {onpointerdown} onresize={onclose} />

<div
	bind:this={popover}
	class={{ popover: true, ready: position.ready, left: position.side === 'left' }}
	style:left="{position.left}px"
	style:top="{position.top}px"
	style:--cal={calendarColor(details.calendar.color)}
	role="dialog"
	aria-labelledby="popover-title"
>
	<h2 id="popover-title" class="title">{details.title}</h2>
	{#if details.location}
		<p class="location">{details.location}</p>
	{/if}

	<p class="when tabular">
		{#each details.when as line, i (i)}
			<span>{line}</span>
		{/each}
	</p>

	<p class="calendar">
		<span class="dot" aria-hidden="true"></span>
		{details.calendar.name}
	</p>

	{#if details.notes}
		<p class="notes">{details.notes}</p>
	{/if}

	<footer>
		{#if details.calendar.editable}
			<button type="button" class="button danger">Delete</button>
			<button type="button" class="button primary">Edit</button>
		{:else}
			<p class="read-only">{readOnlyReason}</p>
		{/if}
	</footer>
</div>

<style>
	.popover {
		position: fixed;
		z-index: 10;
		width: 288px;
		padding: var(--space-lg);
		border-radius: var(--radius-lg);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-2);
		visibility: hidden;
		transform-origin: left top;
		-webkit-user-select: text;
		user-select: text;
	}

	.popover.left {
		transform-origin: right top;
	}

	.popover.ready {
		visibility: visible;
		animation: popover-in var(--duration-slow) var(--ease-standard) both;
	}

	@keyframes popover-in {
		from {
			opacity: 0;
			transform: scale(0.96);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	p {
		margin: 0;
	}

	.title {
		margin: 0;
		font-size: var(--text-headline);
		line-height: var(--leading-headline);
		font-weight: var(--weight-semibold);
		overflow-wrap: anywhere;
	}

	.location {
		color: var(--text-secondary);
	}

	.when {
		display: flex;
		flex-direction: column;
		margin-top: var(--space-md);
	}

	.calendar {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		margin-top: var(--space-md);
		color: var(--text-secondary);
	}

	.dot {
		width: 8px;
		height: 8px;
		border-radius: var(--radius-full);
		background: var(--cal);
	}

	.notes {
		margin-top: var(--space-md);
		padding-top: var(--space-md);
		border-top: 1px solid var(--separator);
	}

	footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-sm);
		margin-top: var(--space-lg);
	}

	.read-only {
		flex: 1;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.button {
		height: var(--control-height);
		padding: 0 var(--space-md);
		border: 0;
		border-radius: var(--radius-md);
		font-weight: var(--weight-medium);
		transition:
			background-color var(--duration-fast) var(--ease-standard),
			filter var(--duration-fast) var(--ease-standard);
	}

	.danger {
		margin-right: auto;
		background: transparent;
		color: var(--danger);
	}

	.danger:hover {
		background: var(--fill-hover);
	}

	.primary {
		background: var(--accent);
		color: var(--on-accent);
	}

	.primary:hover {
		filter: brightness(1.08);
	}
</style>
