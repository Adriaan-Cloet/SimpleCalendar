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
	}

	let { title, view, onview, onprevious, ontoday, onnext, onnew, sync, onsync, onaccounts }: Props =
		$props();

	// Month comes with #24; until then it shows, disabled, where it will be
	const views: { value: View | 'month'; label: string }[] = [
		{ value: 'day', label: 'Day' },
		{ value: 'week', label: 'Week' },
		{ value: 'month', label: 'Month' }
	];
	const unit = $derived(view === 'day' ? 'day' : 'week');
</script>

<header class="toolbar">
	<h1 class="title">{title}</h1>

	<div class="segmented" role="radiogroup" aria-label="View">
		{#each views as option (option.value)}
			<button
				type="button"
				role="radio"
				aria-checked={option.value === view}
				class={{ segment: true, selected: option.value === view }}
				disabled={option.value === 'month'}
				onclick={() => option.value !== 'month' && onview(option.value)}>{option.label}</button
			>
		{/each}
	</div>

	<div class="actions">
		<SyncIndicator summary={sync} {onsync} {onaccounts} />
		<div class="nav">
			<button type="button" class="icon-button" aria-label="Previous {unit}" onclick={onprevious}>
				<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
					<path d="M10 3.5 5.5 8l4.5 4.5" />
				</svg>
			</button>
			<button type="button" class="button" onclick={ontoday}>Today</button>
			<button type="button" class="icon-button" aria-label="Next {unit}" onclick={onnext}>
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

	.title {
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

	.segmented {
		display: flex;
		padding: var(--space-2xs);
		border-radius: var(--radius-md);
		background: var(--fill-control);
	}

	.segment {
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

	.segment:not(.selected):not(:disabled):hover {
		background: var(--fill-hover);
	}

	.segment:disabled {
		color: var(--text-tertiary);
	}

	.segment.selected {
		background: var(--control-thumb);
		box-shadow: var(--shadow-1);
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
