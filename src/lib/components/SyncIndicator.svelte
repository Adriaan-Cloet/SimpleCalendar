<script lang="ts">
	// The toolbar's sync status (issue #22): syncing, up to date, offline, failed, or an account
	// that needs signing in again. A click syncs everything now, or opens Accounts when the
	// problem needs the user there.
	import type { SyncSummary } from '#lib/sync-summary.js';

	interface Props {
		summary: SyncSummary;
		/** Syncs every source now */
		onsync: () => void;
		/** Opens the Accounts dialog */
		onaccounts: () => void;
	}

	let { summary, onsync, onaccounts }: Props = $props();

	const problem = $derived(summary.kind === 'error' || summary.kind === 'auth_required');
</script>

{#if summary.kind !== 'none'}
	<button
		type="button"
		class={{ indicator: true, problem, labelled: summary.label !== null }}
		title={summary.title}
		aria-label={summary.label ? `${summary.label}. ${summary.title}` : summary.title}
		onclick={() => (summary.opensAccounts ? onaccounts() : onsync())}
	>
		<svg
			class={{ spinning: summary.kind === 'syncing' }}
			viewBox="0 0 16 16"
			width="16"
			height="16"
			aria-hidden="true"
		>
			{#if summary.kind === 'offline'}
				<!-- A cloud, struck through -->
				<path d="M4.5 12.5h7a3 3 0 0 0 .4-5.97A4 4 0 0 0 4.3 6.6 3 3 0 0 0 4.5 12.5z" />
				<path d="M2.5 2.5l11 11" />
			{:else if problem}
				<circle cx="8" cy="8" r="6" />
				<path d="M8 4.75v3.75" />
				<circle class="filled" cx="8" cy="11" r="0.9" />
			{:else}
				<path d="M13 8a5 5 0 1 1-1.46-3.54" />
				<path d="M13 2.5v3h-3" />
			{/if}
		</svg>
		{#if summary.label}
			<span class="label">{summary.label}</span>
		{/if}
	</button>
{/if}

<style>
	.indicator {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: var(--space-xs);
		min-width: var(--control-height);
		height: var(--control-height);
		padding: 0;
		border: 0;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text-secondary);
		white-space: nowrap;
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.indicator.labelled {
		padding: 0 var(--space-sm);
	}

	.indicator.problem {
		color: var(--danger);
	}

	.indicator:hover {
		background: var(--fill-hover);
	}

	.indicator:active {
		background: var(--fill-pressed);
	}

	.label {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-medium);
	}

	svg {
		flex: none;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.filled {
		fill: currentColor;
		stroke: none;
	}

	.spinning {
		animation: spin var(--duration-spin) linear infinite;
	}

	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	/* The tooltip still says "Syncing…" */
	@media (prefers-reduced-motion: reduce) {
		.spinning {
			animation: none;
		}
	}
</style>
