<script lang="ts">
	// The notice after `sync:conflict` (issue #21): a change met a newer version on the server,
	// which was kept. "Show" opens the list of changes not saved. The page decides when it goes.
	import { conflictNotice } from '#lib/sync-issue-text.js';

	interface Props {
		/** The titles of every conflict since the notice appeared */
		titles: string[];
		onshow: () => void;
		onclose: () => void;
	}

	let { titles, onshow, onclose }: Props = $props();

	const text = $derived(conflictNotice(titles));
</script>

<div class="notice" role="status">
	<div class="text">
		<span class="title">{text.title}</span>
		<span class="detail">{text.detail}</span>
	</div>
	<button type="button" class="button" onclick={onshow}>Show</button>
	<button type="button" class="icon-button" aria-label="Close" onclick={onclose}>
		<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
			<path d="M4 4l8 8M12 4l-8 8" />
		</svg>
	</button>
</div>

<style>
	.notice {
		position: fixed;
		left: 50%;
		bottom: var(--space-xl);
		z-index: 20;
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		width: max-content;
		max-width: min(440px, calc(100vw - 2 * var(--space-xl)));
		padding: var(--space-sm) var(--space-sm) var(--space-sm) var(--space-lg);
		border-radius: var(--radius-lg);
		color: var(--text);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-3);
		transform: translateX(-50%);
		animation: notice-in var(--duration-slow) var(--ease-standard) both;
	}

	@keyframes notice-in {
		from {
			opacity: 0;
			transform: translate(-50%, var(--space-sm));
		}
		to {
			opacity: 1;
			transform: translateX(-50%);
		}
	}

	.text {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}

	.title {
		font-weight: var(--weight-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.detail {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.button,
	.icon-button {
		display: inline-flex;
		flex: none;
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
	}
</style>
