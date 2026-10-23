<script lang="ts">
	import SyncIndicator from '#lib/components/SyncIndicator.svelte';
	import type { SyncSummary } from '#lib/sync-summary.js';
	import type { View } from '#lib/view-range.js';

	interface Props {
		title: string;
		view: View;
		onview: (view: View) => void;
		onprevious: () => void;
		ontoday: () => void;
		onnext: () => void;
		onnew: () => void;
		/** What sync is doing, for the indicator */
		sync: SyncSummary;
		onsync: () => void;
		onaccounts: () => void;
		/** The month to go back to, on a day opened from the month view: its name and the action */
		back?: { label: string; onback: () => void };
	}

	let {
		title,
		view,
		onview,
		onprevious,
		ontoday,
		onnext,
		onnew,
		sync,
		onsync,
		onaccounts,
		back
	}: Props = $props();

	const views: { value: View; label: string }[] = [
		{ value: 'day', label: 'Day' },
		{ value: 'week', label: 'Week' },
		{ value: 'month', label: 'Month' }
	];
	const selectedIndex = $derived(views.findIndex((option) => option.value === view));
</script>

<header class="toolbar">
	<div class="heading">
		{#if back}
			<button
				type="button"
				class="back"
				aria-label="Back to {back.label}"
				title="Back to the month (Esc)"
				onclick={back.onback}
			>
				<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
					<path d="M10 3.5 5.5 8l4.5 4.5" />
				</svg>
				{back.label}
			</button>
		{/if}
		<h1 class="title">{title}</h1>
	</div>

	<div class="segmented" role="radiogroup" aria-label="View" style:--count={views.length}>
		<!-- The white thumb slides to the chosen view -->
		<span class="thumb" style:--index={selectedIndex} aria-hidden="true"></span>
		{#each views as option (option.value)}
			<button
				type="button"
				role="radio"
				aria-checked={option.value === view}
				class={{ segment: true, selected: option.value === view }}
				onclick={() => onview(option.value)}>{option.label}</button
			>
		{/each}
	</div>

	<div class="actions">
		<SyncIndicator summary={sync} {onsync} {onaccounts} />
		<div class="nav">
			<button type="button" class="icon-button" aria-label="Previous {view}" onclick={onprevious}>
				<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
					<path d="M10 3.5 5.5 8l4.5 4.5" />
				</svg>
			</button>
			<button type="button" class="button" onclick={ontoday}>Today</button>
			<button type="button" class="icon-button" aria-label="Next {view}" onclick={onnext}>
				<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
					<path d="M6 3.5 10.5 8 6 12.5" />
				</svg>
			</button>
		</div>
		<button
			type="button"
			class="icon-button"
			aria-label="New event"
			title="New event"
			onclick={onnew}
		>
			<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
				<path d="M8 3v10M3 8h10" />
			</svg>
		</button>
	</div>
</header>

<style>
	.toolbar {
		display: grid;
		grid-template-columns: 1fr auto 1fr;
		align-items: center;
		gap: var(--space-lg);
		height: var(--toolbar-height);
		padding: 0 var(--space-lg) 0 var(--space-xl);
	}

	.heading {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		min-width: 0;
	}

	/* Back to the month, before the day's heading: quiet, in the accent colour */
	.back {
		display: inline-flex;
		flex: none;
		align-items: center;
		gap: var(--space-2xs);
		height: var(--control-height);
		padding: 0 var(--space-sm) 0 var(--space-xs);
		border: 0;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--accent-text);
		font-weight: var(--weight-medium);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.back:hover {
		background: var(--fill-hover);
	}

	.back:active {
		background: var(--fill-pressed);
	}

	.title {
		min-width: 0;
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--text-title);
		line-height: var(--leading-title);
		font-weight: var(--weight-semibold);
		letter-spacing: -0.01em;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	/* Equal segments, so the thumb can slide by its own width */
	.segmented {
		position: relative;
		display: grid;
		grid-template-columns: repeat(var(--count), 1fr);
		padding: var(--space-2xs);
		border-radius: var(--radius-md);
		background: var(--fill-control);
	}

	.thumb {
		position: absolute;
		top: var(--space-2xs);
		bottom: var(--space-2xs);
		left: var(--space-2xs);
		width: calc((100% - 2 * var(--space-2xs)) / var(--count));
		border-radius: var(--radius-sm);
		background: var(--control-thumb);
		box-shadow: var(--shadow-1);
		transform: translateX(calc(var(--index) * 100%));
		transition: transform var(--duration-base) var(--ease-standard);
	}

	.segment {
		position: relative;
		min-width: 64px;
		height: calc(var(--control-height) - 2 * var(--space-2xs));
		padding: 0 var(--space-md);
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		font-size: var(--text-footnote);
		font-weight: var(--weight-medium);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.segment:not(.selected):hover {
		background: var(--fill-hover);
	}

	.actions {
		display: flex;
		justify-content: flex-end;
		align-items: center;
		gap: var(--space-md);
	}

	.nav {
		display: flex;
		align-items: center;
		gap: var(--space-2xs);
	}

	.button,
	.icon-button {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		height: var(--control-height);
		border-radius: var(--radius-md);
		background: transparent;
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.button {
		padding: 0 var(--space-md);
		border: 1px solid var(--separator);
		font-weight: var(--weight-medium);
	}

	.icon-button {
		width: var(--control-height);
		border: 0;
		color: var(--text-secondary);
	}

	.button:hover,
	.icon-button:hover {
		background: var(--fill-hover);
	}

	.button:active,
	.icon-button:active {
		background: var(--fill-pressed);
	}

	svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
</style>
