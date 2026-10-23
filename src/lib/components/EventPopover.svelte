<script lang="ts" module>
	import type { CalendarInfo, Scope } from '#lib/calendar.js';

	export interface EventDetails {
		title: string;
		calendar: CalendarInfo;
		/** The event's own colour, else its calendar's */
		color: string | null;
		editable: boolean;
		/** An occurrence of a series: deleting asks whether this event or all. `null` while the
		 * event is still being read, which keeps "Delete" off: for a single event it deletes all. */
		recurring: boolean | null;
		/** One line per entry, e.g. the date, the time range and how it repeats */
		when: string[];
		location?: string;
		/** Plain text from `get_event`; shown as text, never as HTML */
		notes?: string;
	}
</script>

<script lang="ts">
	import { calendarColor } from '#lib/calendar-color.js';
	import { serviceError } from '#lib/sources.js';

	interface Props {
		details: EventDetails;
		anchor: HTMLElement;
		onclose: () => void;
		onedit: () => void;
		/** Deletes the event, or for a series this occurrence or all; rejects with a `ServiceError` */
		ondelete: (scope: Scope) => Promise<void>;
	}

	let { details, anchor, onclose, onedit, ondelete }: Props = $props();

	/** "Delete" was pressed once and waits for confirmation */
	let confirming = $state(false);
	let deleting = $state(false);
	let error = $state<string | null>(null);

	async function remove(scope: Scope) {
		if (deleting) return;
		deleting = true;
		error = null;
		try {
			await ondelete(scope);
		} catch (e) {
			error = serviceError(e).message;
			deleting = false;
		}
	}

	let popover: HTMLElement | undefined = $state();
	type Side = 'right' | 'left' | 'below' | 'above';
	let position = $state({ left: 0, top: 0, side: 'right' as Side, ready: false });

	/** Between the popover and its event */
	const GAP = 8;
	/** At least this far from every edge of the window: `--space-xl` */
	const EDGE = 24;
	/** How far in from a wide event's left edge the popover starts when it goes below or above */
	const INDENT = 16;

	// Beside the event if it fits: right, else left. An event too wide for that, such as one in
	// the day view, gets it below, else above, else over its right end, so it never covers the
	// sidebar.
	function place(popover: HTMLElement) {
		const rect = anchor.getBoundingClientRect();
		const width = popover.offsetWidth;
		const height = popover.offsetHeight;
		const right = window.innerWidth - EDGE;
		const bottom = window.innerHeight - EDGE;
		const clampTop = (top: number) =>
			Math.min(Math.max(EDGE, top), Math.max(EDGE, bottom - height));
		if (rect.right + GAP + width <= right) {
			position = { left: rect.right + GAP, top: clampTop(rect.top), side: 'right', ready: true };
			return;
		}
		if (rect.left - GAP - width >= EDGE) {
			const left = rect.left - GAP - width;
			position = { left, top: clampTop(rect.top), side: 'left', ready: true };
			return;
		}
		const left = Math.max(EDGE, Math.min(rect.left + INDENT, right - width));
		const below = rect.bottom + GAP;
		const above = rect.top - GAP - height;
		if (below + height <= bottom) {
			position = { left, top: below, side: 'below', ready: true };
		} else if (above >= EDGE) {
			position = { left, top: above, side: 'above', ready: true };
		} else {
			// No room either way: over the event's right end, level with it, so its title on the
			// left stays visible
			const over = Math.max(left, Math.min(rect.right - INDENT, right) - width);
			position = { left: over, top: clampTop(rect.top), side: 'left', ready: true };
		}
	}

	// Placed again whenever it changes size: the notes arrive after it opens, from `get_event`
	$effect(() => {
		if (!popover) return;
		const target = popover;
		place(target);
		const observer = new ResizeObserver(() => place(target));
		observer.observe(target);
		return () => observer.disconnect();
	});

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			// Closing unmounts this popover, after which `anchor` can no longer be read
			const event = anchor;
			onclose();
			event.focus();
		}
	}

	function onpointerdown(e: PointerEvent) {
		const target = e.target as Element;
		// A click on another event selects that event instead; the page handles it
		if (popover?.contains(target) || target.closest('[data-event]')) return;
		onclose();
	}

	const readOnlyReason = $derived.by(() => {
		if (details.calendar.source_kind === 'ics') return 'Subscribed calendars can’t be edited.';
		if (details.calendar.editable) return 'Only the organiser can change this event.';
		return 'You can view this calendar but not change it.';
	});
</script>

<svelte:window {onkeydown} {onpointerdown} onresize={onclose} />

<div
	bind:this={popover}
	class={{ popover: true, ready: position.ready, [position.side]: true }}
	style:left="{position.left}px"
	style:top="{position.top}px"
	style:--cal={calendarColor(details.color, details.calendar.id)}
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

	{#if error}
		<p class="error" role="alert">{error}</p>
	{/if}

	<footer>
		{#if details.editable && details.recurring && confirming}
			<p class="confirm own-line">Delete only this event, or all events in the series?</p>
			<button type="button" class="button quiet" onclick={() => (confirming = false)}>
				Cancel
			</button>
			<button
				type="button"
				class="button danger-strong"
				disabled={deleting}
				onclick={() => remove('this')}
			>
				This event
			</button>
			<button
				type="button"
				class="button danger-strong"
				disabled={deleting}
				onclick={() => remove('all')}
			>
				All events
			</button>
		{:else if details.editable && confirming}
			<p class="confirm">Delete this event?</p>
			<button type="button" class="button quiet" onclick={() => (confirming = false)}>
				Cancel
			</button>
			<button
				type="button"
				class="button danger-strong"
				disabled={deleting}
				onclick={() => remove('all')}
			>
				Delete
			</button>
		{:else if details.editable}
			<button
				type="button"
				class="button danger"
				disabled={details.recurring === null}
				onclick={() => (confirming = true)}
			>
				Delete
			</button>
			<button type="button" class="button primary" onclick={onedit}>Edit</button>
		{:else}
			<p class="read-only">{readOnlyReason}</p>
		{/if}
	</footer>
</div>

<style>
	.popover {
		position: fixed;
		z-index: 10;
		display: flex;
		flex-direction: column;
		width: min(360px, calc(100vw - 2 * var(--space-xl)));
		/* The same distance from the window's edges as `EDGE` in the placement */
		max-height: calc(100vh - 2 * var(--space-xl));
		padding: var(--space-lg) var(--space-xl) var(--space-lg);
		font-size: var(--text-headline);
		line-height: var(--leading-headline);
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

	.popover.above {
		transform-origin: left bottom;
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
		flex: none;
		margin: 0;
		font-size: var(--text-title);
		line-height: var(--leading-title);
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

	/* The notes take what room the window leaves and scroll inside it, on a soft panel so long
	   text reads apart from the details above */
	.notes {
		flex: 0 1 auto;
		min-height: 80px;
		max-height: 420px;
		margin-top: var(--space-lg);
		padding: var(--space-md);
		border-radius: var(--radius-md);
		background: var(--fill-hover);
		overflow-y: auto;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}

	.error {
		margin-top: var(--space-md);
		color: var(--danger);
	}

	.confirm {
		flex: 1;
		align-self: center;
	}

	/* A longer question goes above its answers */
	.confirm.own-line {
		flex-basis: 100%;
	}

	footer {
		flex: none;
		display: flex;
		flex-wrap: wrap;
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

	.danger:hover,
	.quiet:hover {
		background: var(--fill-hover);
	}

	.quiet {
		background: transparent;
		color: var(--text-secondary);
	}

	/* The confirmed delete: red text on a faint fill, so it reads as the answer */
	.danger-strong {
		background: var(--fill-hover);
		color: var(--danger);
	}

	.danger-strong:hover {
		background: var(--fill-pressed);
	}

	.button:disabled {
		opacity: 0.4;
		pointer-events: none;
	}

	.primary {
		background: var(--accent);
		color: var(--on-accent);
	}

	.primary:hover {
		filter: brightness(1.08);
	}
</style>
