<script lang="ts">
	// Add, refresh and remove ICS subscriptions (issue #9) and Google accounts (#13). The feed
	// address and the Google sign-in token go straight to the keychain in Rust; this dialog only
	// ever shows the host or the account's address.
	import { displayLocale } from '#lib/display-format.js';
	import {
		addIcsSource,
		beginGoogleSignIn,
		cancelGoogleSignIn,
		getGoogleClient,
		getStatus,
		setGoogleClient,
		listSources,
		onSyncStatus,
		removeSource,
		serviceError,
		syncNow,
		type GoogleClientInfo,
		type SecretsUnavailable,
		type SourceInfo
	} from '#lib/sources.js';

	interface Props {
		open: boolean;
	}

	let { open = $bindable() }: Props = $props();

	let dialog: HTMLDialogElement | undefined = $state();
	let sources = $state<SourceInfo[]>([]);
	/** Sources with a sync in progress, from `sync:status` */
	let syncing = $state<Record<string, boolean>>({});
	let keyring = $state<SecretsUnavailable | null>(null);
	/** The source whose "Remove" waits for confirmation */
	let confirming = $state<string | null>(null);
	/** A failed list, refresh or remove */
	let listError = $state<string | null>(null);

	let url = $state('');
	let name = $state('');
	let adding = $state(false);
	let addError = $state<string | null>(null);
	let checking = $state(false);

	/** The Google sign-in waiting for the browser: `new` for a new account, else a source id */
	let signingIn = $state<string | null>(null);
	let googleError = $state<string | null>(null);
	/** The form for the user's own OAuth client, when shown */
	let client = $state<GoogleClientInfo | null>(null);
	let clientId = $state('');
	let clientSecret = $state('');
	let savingClient = $state(false);
	let clientError = $state<string | null>(null);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) dialog.showModal();
		else if (!open && dialog.open) dialog.close();
	});

	// While open: load the list and follow sync progress
	$effect(() => {
		if (!open) return;
		confirming = null;
		listError = null;
		addError = null;
		googleError = null;
		client = null;
		void refreshStatus();
		void reload();
		const unlisten = onSyncStatus((status) => {
			syncing[status.source_id] = status.state === 'syncing';
			if (status.state !== 'syncing') void reload();
		}).catch(() => null);
		return () => {
			void unlisten.then((stop) => stop?.());
			// Closing the dialog gives up a sign-in that still waits for the browser
			if (signingIn) void cancelGoogleSignIn().catch(() => undefined);
		};
	});

	async function reload() {
		try {
			sources = await listSources();
		} catch (e) {
			listError = serviceError(e).message;
		}
	}

	async function refreshStatus() {
		checking = true;
		try {
			keyring = (await getStatus()).secrets_unavailable;
		} catch {
			keyring = null;
		} finally {
			checking = false;
		}
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		if (adding || !url.trim()) return;
		adding = true;
		addError = null;
		try {
			const source = await addIcsSource(url, name);
			sources = [...sources, source];
			// The engine starts its first sync at once; its events confirm it
			syncing[source.id] = true;
			url = '';
			name = '';
		} catch (e) {
			const error = serviceError(e);
			if (error.kind === 'secrets_unavailable') keyring = error;
			else addError = error.message;
		} finally {
			adding = false;
		}
	}

	async function refresh(source: SourceInfo) {
		syncing[source.id] = true;
		try {
			await syncNow(source.id);
		} catch (e) {
			syncing[source.id] = false;
			listError = serviceError(e).message;
		}
	}

	async function remove(source: SourceInfo) {
		confirming = null;
		try {
			await removeSource(source.id);
			sources = sources.filter((s) => s.id !== source.id);
		} catch (e) {
			const error = serviceError(e);
			if (error.kind === 'secrets_unavailable') keyring = error;
			else listError = error.message;
		}
	}

	async function signIn(source: SourceInfo | null) {
		if (signingIn) return;
		signingIn = source?.id ?? 'new';
		googleError = null;
		try {
			await beginGoogleSignIn(source?.id ?? null);
			await reload();
		} catch (e) {
			const error = serviceError(e);
			if (error.kind === 'cancelled') return;
			// Without a client the form below explains what to enter
			if (error.kind === 'google_client_missing') await showClientForm();
			else if (error.kind === 'secrets_unavailable') keyring = error;
			else googleError = error.message;
		} finally {
			signingIn = null;
		}
	}

	async function showClientForm() {
		clientError = null;
		try {
			client = await getGoogleClient();
			clientId = client.custom_client_id ?? '';
			clientSecret = '';
		} catch (e) {
			googleError = serviceError(e).message;
		}
	}

	/** Saves the form, or with `builtIn` goes back to the built-in client */
	async function saveClient(event: SubmitEvent | null, builtIn = false) {
		event?.preventDefault();
		if (savingClient) return;
		savingClient = true;
		clientError = null;
		try {
			await setGoogleClient(builtIn ? '' : clientId, builtIn ? '' : clientSecret);
			client = null;
			googleError = null;
		} catch (e) {
			const error = serviceError(e);
			if (error.kind === 'secrets_unavailable') keyring = error;
			else clientError = error.message;
		} finally {
			savingClient = false;
		}
	}

	const relative = new Intl.RelativeTimeFormat(displayLocale, { numeric: 'auto' });

	function updated(seconds: number): string {
		const minutes = Math.round((seconds * 1000 - Date.now()) / 60_000);
		if (minutes > -1) return 'Updated just now';
		if (minutes > -60) return `Updated ${relative.format(minutes, 'minute')}`;
		const hours = Math.round(minutes / 60);
		if (hours > -24) return `Updated ${relative.format(hours, 'hour')}`;
		return `Updated ${relative.format(Math.round(hours / 24), 'day')}`;
	}

	function statusLine(source: SourceInfo): { text: string; problem: boolean } {
		if (syncing[source.id]) return { text: 'Syncing…', problem: false };
		if (source.state === 'auth_required') {
			return { text: 'Sign in again to keep this account in sync.', problem: true };
		}
		if (source.state !== 'ok' && source.last_error) {
			// The core's messages start in lower case ("the server answered HTTP 404")
			const text = source.last_error;
			return { text: text.charAt(0).toUpperCase() + text.slice(1), problem: true };
		}
		if (source.last_synced_at === null) return { text: 'Not synced yet', problem: false };
		return { text: updated(source.last_synced_at), problem: false };
	}
</script>

<dialog
	bind:this={dialog}
	class="dialog"
	aria-labelledby="subscriptions-title"
	onclose={() => (open = false)}
>
	<header>
		<h2 id="subscriptions-title" class="title">Accounts</h2>
		<button type="button" class="icon-button" aria-label="Close" onclick={() => (open = false)}>
			<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
				<path d="M4 4l8 8M12 4l-8 8" />
			</svg>
		</button>
	</header>

	{#if keyring}
		<div class="keyring" role="alert">
			<p>{keyring.message}</p>
			<button type="button" class="button" disabled={checking} onclick={refreshStatus}>
				Try again
			</button>
		</div>
	{/if}

	{#if sources.length === 0}
		<p class="empty">No accounts yet. Sign in with Google or add a calendar feed below.</p>
	{:else}
		<ul class="sources">
			{#each sources as source (source.id)}
				{@const status = statusLine(source)}
				<li class="source">
					<div class="details">
						<span class="name">{source.display_name}</span>
						<span class="meta">
							{#if source.account_label && source.account_label !== source.display_name}
								<span class="host">{source.account_label}</span>
								<span aria-hidden="true">·</span>
							{/if}
							<span class={{ status: true, problem: status.problem }}>{status.text}</span>
						</span>
						{#each source.warnings as warning (warning)}
							<span class="warning">{warning}</span>
						{/each}
					</div>
					<div class="row-actions">
						{#if confirming === source.id}
							<button type="button" class="button" onclick={() => (confirming = null)}>
								Cancel
							</button>
							<button type="button" class="button danger" onclick={() => remove(source)}>
								Remove
							</button>
						{:else}
							{#if source.kind === 'google' && source.state === 'auth_required'}
								<button
									type="button"
									class="button"
									disabled={signingIn !== null || keyring !== null}
									onclick={() => signIn(source)}
								>
									{signingIn === source.id ? 'Waiting for browser…' : 'Sign in again'}
								</button>
							{/if}
							<button
								type="button"
								class="icon-button"
								aria-label="Refresh {source.display_name}"
								title="Refresh"
								disabled={syncing[source.id]}
								onclick={() => refresh(source)}
							>
								<svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
									<path d="M13 8a5 5 0 1 1-1.46-3.54" />
									<path d="M13 2.5v3h-3" />
								</svg>
							</button>
							<button type="button" class="button quiet" onclick={() => (confirming = source.id)}>
								Remove
							</button>
						{/if}
					</div>
				</li>
			{/each}
		</ul>
	{/if}

	{#if listError}
		<p class="error" role="alert">{listError}</p>
	{/if}

	<section class="add" aria-labelledby="google-title">
		<h3 id="google-title" class="section-title">Add a Google account</h3>
		<p class="hint">
			Sign in with Google in your browser. Only a sign-in token is kept, in the system keyring.
		</p>
		{#if googleError}
			<p class="error" role="alert">{googleError}</p>
		{/if}
		{#if client}
			<form class="client" onsubmit={saveClient}>
				<p class="hint">
					{#if !client.built_in}This build has no Google client built in.{/if}
					Use your own Google Cloud project: create an OAuth client of type “Desktop app” with the Google
					Calendar API enabled, and enter its id and secret.
				</p>
				<label class="field">
					<span class="label">Client ID</span>
					<input
						type="text"
						bind:value={clientId}
						placeholder="….apps.googleusercontent.com"
						autocomplete="off"
						spellcheck="false"
						required
					/>
				</label>
				<label class="field">
					<span class="label">Client secret</span>
					<input type="password" bind:value={clientSecret} autocomplete="off" />
				</label>
				{#if clientError}
					<p class="error" role="alert">{clientError}</p>
				{/if}
				<div class="form-actions">
					{#if client.built_in && client.custom_client_id}
						<button
							type="button"
							class="button quiet leading"
							disabled={savingClient}
							onclick={() => saveClient(null, true)}
						>
							Use built-in client
						</button>
					{/if}
					<button type="button" class="button quiet" onclick={() => (client = null)}>
						Cancel
					</button>
					<button
						type="submit"
						class="button"
						disabled={savingClient || keyring !== null || !clientId.trim()}
					>
						Save
					</button>
				</div>
			</form>
		{/if}
		{#if !client}
			<div class="form-actions">
				<button type="button" class="button quiet leading" onclick={showClientForm}>
					Use your own client…
				</button>
				{#if signingIn === 'new'}
					<span class="waiting">Waiting for your browser…</span>
					<button type="button" class="button" onclick={() => cancelGoogleSignIn()}>Cancel</button>
				{:else}
					<button
						type="button"
						class="button"
						disabled={signingIn !== null || keyring !== null}
						onclick={() => signIn(null)}
					>
						Sign in with Google
					</button>
				{/if}
			</div>
		{/if}
	</section>

	<form class="add" onsubmit={add}>
		<h3 class="section-title">Add a subscription</h3>
		<label class="field">
			<span class="label">Feed address</span>
			<input
				type="url"
				bind:value={url}
				placeholder="https:// or webcal://"
				autocomplete="off"
				spellcheck="false"
				required
			/>
		</label>
		<label class="field">
			<span class="label">Name</span>
			<input type="text" bind:value={name} placeholder="Optional" autocomplete="off" />
		</label>
		<p class="hint">
			The address is kept in the system keyring, never in a file. Private addresses work too.
		</p>
		{#if addError}
			<p class="error" role="alert">{addError}</p>
		{/if}
		<div class="form-actions">
			<button
				type="submit"
				class="button primary"
				disabled={adding || keyring !== null || !url.trim()}
			>
				{adding ? 'Subscribing…' : 'Subscribe'}
			</button>
		</div>
	</form>
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

	.section-title {
		margin: 0;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
		color: var(--text-secondary);
	}

	/* Keyring unavailable */
	.keyring {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-sm);
		margin-bottom: var(--space-lg);
		padding: var(--space-md);
		border-radius: var(--radius-md);
		background: var(--fill-hover);
		-webkit-user-select: text;
		user-select: text;
	}

	/* Source list */
	.empty {
		padding: var(--space-md) 0;
		color: var(--text-secondary);
	}

	.sources {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.source {
		display: flex;
		align-items: center;
		gap: var(--space-md);
		padding: var(--space-sm) 0;
		border-bottom: 1px solid var(--separator);
	}

	.source:last-child {
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

	.meta {
		display: flex;
		gap: var(--space-xs);
		min-width: 0;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.host,
	.status {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.host {
		flex: none;
		max-width: 50%;
	}

	.status.problem {
		color: var(--danger);
		white-space: normal;
	}

	.warning {
		margin-top: var(--space-2xs);
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.row-actions {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		flex: none;
	}

	/* Add form */
	.add {
		display: flex;
		flex-direction: column;
		gap: var(--space-md);
		margin-top: var(--space-lg);
		padding-top: var(--space-lg);
		border-top: 1px solid var(--separator);
	}

	.field {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
	}

	.label {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-medium);
	}

	input {
		height: var(--control-height);
		padding: 0 var(--space-sm);
		border: 1px solid var(--separator);
		border-radius: var(--radius-md);
		background: var(--surface);
		-webkit-user-select: text;
		user-select: text;
	}

	input::placeholder {
		color: var(--text-secondary);
	}

	input:focus-visible {
		outline-offset: 0;
	}

	.hint {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.error {
		margin-top: var(--space-sm);
		color: var(--danger);
	}

	.add .error {
		margin-top: 0;
	}

	.form-actions {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: var(--space-xs);
	}

	/* A secondary action on the left of a row of actions */
	.leading {
		margin-right: auto;
	}

	.client {
		display: flex;
		flex-direction: column;
		gap: var(--space-md);
	}

	.waiting {
		color: var(--text-secondary);
	}

	/* Buttons, as in the toolbar and the event popover */
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
			filter var(--duration-fast) var(--ease-standard),
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

	.button.danger {
		border-color: transparent;
		color: var(--danger);
	}

	.button.primary {
		border-color: transparent;
		color: var(--on-accent);
		background: var(--accent);
	}

	.icon-button {
		width: var(--control-height);
		border: 0;
		color: var(--text-secondary);
		background: transparent;
	}

	.button:not(.primary):hover,
	.icon-button:hover {
		background: var(--fill-hover);
	}

	.button:not(.primary):active,
	.icon-button:active {
		background: var(--fill-pressed);
	}

	.button.primary:hover {
		filter: brightness(1.08);
	}

	.button:disabled,
	.icon-button:disabled {
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
