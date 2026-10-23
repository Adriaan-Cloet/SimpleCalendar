<script lang="ts">
	// Changes the server did not save (issue #21). Redo makes one again as a new edit on the
	// event as it is now; Dismiss keeps the server's version. Rust decides what redo does and
	// whether it can (`can_redo`).
	import { serviceError } from '#lib/sources.js';
	import { issueReason, issueSummary, issueTime } from '#lib/sync-issue-text.js';
	import { dismissSyncIssue, redoSyncIssue, type SyncIssue } from '#lib/sync-issues.js';

	interface Props {
		open: boolean;
		issues: SyncIssue[];
		/** After a redo or dismiss: read the list again */
		onchanged: () => void;
	}

	let { open = $bindable(), issues, onchanged }: Props = $props();

	let dialog: HTMLDialogElement | undefined = $state();
	/** The issue whose redo or dismiss is running */
	let busy = $state<number | null>(null);
	/** A refused redo or dismiss, per issue */
	let errors = $state<Record<number, string>>({});

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			errors = {};
			dialog.showModal();
		} else if (!open && dialog.open) dialog.close();
	});

	async function act(issue: SyncIssue, action: (id: number) => Promise<void>) {
		busy = issue.id;
		delete errors[issue.id];
		try {
			await action(issue.id);
		} catch (e) {
			errors[issue.id] = serviceError(e).message;
		} finally {
			busy = null;
			onchanged();
		}
	}
</script>

<dialog
	bind:this={dialog}
	class="dialog"
	aria-labelledby="sync-issues-title"
	onclose={() => (open = false)}
>
	<header>
		<h2 id="sync-issues-title" class="title">Changes not saved</h2>
		<button type="button" class="icon-button" aria-label="Close" onclick={() => (open = false)}>
			<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
				<path d="M4 4l8 8M12 4l-8 8" />
			</svg>
		</button>
	</header>

	{#if issues.length === 0}
		<p class="empty">All your changes are saved.</p>
	{:else}
		<p class="intro">
			These changes did not reach the calendar. Redo makes a change again on the event as it is now;
			Dismiss leaves the event as it is.
		</p>
		<ul class="issues">
			{#each issues as issue (issue.id)}
				<li class="issue">
					<div class="details">
						<span class="name">{issueSummary(issue)}</span>
						<span class="meta">{issue.calendar_name} · {issueTime(issue.created_at)}</span>
						<span class="reason">{issueReason(issue)}</span>
						{#if errors[issue.id]}
							<span class="error" role="alert">{errors[issue.id]}</span>
						{/if}
					</div>
					<div class="row-actions">
						<button
							type="button"
							class="button quiet"
							disabled={busy !== null}
							onclick={() => act(issue, dismissSyncIssue)}
						>
							Dismiss
						</button>
						{#if issue.can_redo}
							<button
								type="button"
								class="button"
								disabled={busy !== null}
								onclick={() => act(issue, redoSyncIssue)}
							>
								Redo
							</button>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
	{/if}
</dialog>

<style>
	.dialog {
		width: min(480px, calc(100vw - 2 * var(--space-xl)));
		max-height: calc(100vh - 2 * var(--space-2xl));
		padding: var(--space-lg) var(--space-xl) var(--space-xl);
		border: 0;
		border-radius: var(--radius-lg);
		color: var(--text);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-3);
		overflow-y: auto;
	}

	.dialog[open] {
		animation: dialog-in var(--duration-slow) var(--ease-standard) both;
	}

	.dialog::backdrop {
		background: var(--backdrop);
	}

	@keyframes dialog-in {
		from {
			opacity: 0;
			transform: scale(0.96);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: var(--space-md);
	}

	p {
		margin: 0;
	}

	.title {
		margin: 0;
		font-size: var(--text-headline);
		line-height: var(--leading-headline);
		font-weight: var(--weight-semibold);
	}

	.empty,
	.intro {
		color: var(--text-secondary);
	}

	.empty {
		padding: var(--space-md) 0;
	}

	.intro {
		margin-bottom: var(--space-sm);
	}

	.issues {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.issue {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: var(--space-sm) 0;
		border-bottom: 1px solid var(--separator);
	}

	.issue:last-child {
		border-bottom: 0;
	}

	.details {
		display: flex;
		flex-direction: column;
		flex: 1;
		min-width: 0;
	}

	.name {
		font-weight: var(--weight-medium);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.meta,
	.reason,
	.error {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
	}

	.meta {
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.reason {
		margin-top: var(--space-2xs);
		color: var(--text-secondary);
	}

	.error {
		margin-top: var(--space-2xs);
		color: var(--danger);
	}

	.row-actions {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		flex: none;
	}

	/* Buttons, as in the Accounts dialog */
	.button,
	.icon-button {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		height: var(--control-height);
		border-radius: var(--radius-md);
		font-weight: var(--weight-medium);
		transition:
			background-color var(--duration-fast) var(--ease-standard),
			opacity var(--duration-fast) var(--ease-standard);
	}

	.button {
		padding: 0 var(--space-md);
		border: 1px solid var(--separator);
		background: transparent;
	}

	.button.quiet {
		border-color: transparent;
		color: var(--text-secondary);
	}

	.icon-button {
		width: var(--control-height);
		border: 0;
		color: var(--text-secondary);
		background: transparent;
	}

	.button:hover,
	.icon-button:hover {
		background: var(--fill-hover);
	}

	.button:active,
	.icon-button:active {
		background: var(--fill-pressed);
	}

	.button:disabled {
		opacity: 0.4;
		pointer-events: none;
	}

	svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}
</style>
