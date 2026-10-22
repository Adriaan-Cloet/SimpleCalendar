<script lang="ts" module>
	/** Stands for "a new calendar on this computer" while the user has no writable calendar */
	export const NEW_LOCAL_CALENDAR = 0;
</script>

<script lang="ts">
	// Create or edit one event (issue #16). A modal `<dialog>`, like the Accounts dialog: the
	// browser handles focus, Escape and the backdrop in both webviews. The form's rules live in
	// `event-form.ts`; Rust checks everything again when saving.
	import { calendarColor } from '#lib/calendar-color.js';
	import type { CalendarInfo, Scope } from '#lib/calendar.js';
	import {
		calendarChoices,
		formProblem,
		reminderChoices,
		reminderLabel,
		saveScopes,
		withAllDay,
		withRepeatEnd,
		withStart,
		zoneOptions,
		type EventForm
	} from '#lib/event-form.js';
	import { serviceError } from '#lib/sources.js';
	import DateField from './DateField.svelte';
	import TimeField from './TimeField.svelte';

	interface Props {
		/** The form to start from; a copy is edited */
		initial: EventForm;
		/** Whether this creates an event, or changes one */
		isNew: boolean;
		/** It changes an occurrence of a series: saving asks whether for this event or all */
		series: boolean;
		calendars: CalendarInfo[];
		/** The viewer's zone, always offered */
		displayZone: string;
		/** Saves the form for `scope`; rejects with a `ServiceError` */
		onsave: (form: EventForm, scope: Scope) => Promise<void>;
		onclose: () => void;
	}

	let { initial, isNew, series, calendars, displayZone, onsave, onclose }: Props = $props();

	// A copy, taken once: editing never changes the caller's form. The page shows a new editor
	// for every event it opens.
	// svelte-ignore state_referenced_locally
	let form = $state<EventForm>($state.snapshot(initial));
	let saving = $state(false);
	let error = $state<string | null>(null);
	/** What "Save" may apply to while the footer asks; `null` while it does not */
	let asking = $state<Scope[] | null>(null);

	let dialog: HTMLDialogElement | undefined = $state();
	let titleInput: HTMLInputElement | undefined = $state();

	const writable = $derived(calendarChoices(calendars, isNew ? null : initial.calendarId));
	const calendar = $derived(writable.find((c) => c.id === form.calendarId));
	/** Fields whose text cannot be read; the form keeps their last readable value */
	let unreadable = $state({
		startDate: false,
		startTime: false,
		endDate: false,
		endTime: false,
		repeatUntil: false
	});
	const repeats = $derived(form.repeat !== 'never' && form.repeat !== 'custom');
	const problem = $derived.by(() => {
		if (
			unreadable.startDate ||
			unreadable.endDate ||
			(repeats && form.repeatUntil !== null && unreadable.repeatUntil)
		) {
			return 'Enter dates as day/month/year, such as 20/10/2026.';
		}
		// The time fields are hidden while all-day, and their text no longer counts
		if (!form.allDay && (unreadable.startTime || unreadable.endTime)) {
			return 'Enter times on a 24-hour clock, such as 09:30 or 21:30.';
		}
		return formProblem(form);
	});

	// Supported zones from the webview; older engines have no list, and get the two that matter
	const supported =
		typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
	// svelte-ignore state_referenced_locally
	const zones = zoneOptions(supported, form.timeZone, displayZone);

	$effect(() => {
		if (!dialog || dialog.open) return;
		dialog.showModal();
		titleInput?.focus();
	});

	function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving || problem) return;
		if (series) asking = saveScopes(initial, form);
		else save('all');
	}

	async function save(scope: Scope) {
		saving = true;
		error = null;
		try {
			await onsave($state.snapshot(form), scope);
		} catch (e) {
			error = serviceError(e).message;
			asking = null;
		} finally {
			saving = false;
		}
	}

	function setReminder(index: number, minutes: number) {
		form.reminders[index] = { ...form.reminders[index], minutes_before: minutes };
		form.useDefaultReminders = false;
	}

	function removeReminder(index: number) {
		form.reminders.splice(index, 1);
		form.useDefaultReminders = false;
	}

	function addReminder() {
		const used = new Set(form.reminders.map((r) => r.minutes_before));
		const minutes = [10, ...reminderChoices].find((m) => !used.has(m)) ?? 10;
		form.reminders.push({ minutes_before: minutes, method: 'popup' });
		form.useDefaultReminders = false;
	}

	/** The menu's choices, plus a value that came from elsewhere, such as Google */
	function choicesFor(minutes: number): number[] {
		return reminderChoices.includes(minutes)
			? reminderChoices
			: [...reminderChoices, minutes].sort((a, b) => a - b);
	}

	const zoneLabel = (zone: string) => zone.replaceAll('_', ' ');
</script>

<dialog
	bind:this={dialog}
	class="dialog"
	aria-labelledby="editor-title"
	{onclose}
	style:--cal={calendarColor(calendar?.color ?? null, form.calendarId)}
>
	<form onsubmit={submit}>
		<header>
			<h2 id="editor-title" class="heading">{isNew ? 'New event' : 'Edit event'}</h2>
		</header>

		<input
			bind:this={titleInput}
			bind:value={form.title}
			class="title"
			type="text"
			placeholder="New event"
			aria-label="Title"
			autocomplete="off"
		/>
		<input
			bind:value={form.location}
			class="location"
			type="text"
			placeholder="Add location"
			aria-label="Location"
			autocomplete="off"
		/>

		<div class="rows">
			<label class="row" for="editor-all-day">
				<span class="label">All-day</span>
				<input
					id="editor-all-day"
					class="switch"
					type="checkbox"
					checked={form.allDay}
					onchange={(e) => (form = withAllDay(form, e.currentTarget.checked))}
				/>
			</label>

			<div class="row">
				<span class="label" id="editor-starts">Starts</span>
				<div class="when">
					<DateField
						labelledby="editor-starts"
						class="date tabular"
						value={form.startDate}
						onvalue={(date) => (form = withStart(form, date, form.startTime))}
						oninvalid={(bad) => (unreadable.startDate = bad)}
					/>
					{#if !form.allDay}
						<TimeField
							label="Start time"
							class="time tabular"
							value={form.startTime}
							onvalue={(time) => (form = withStart(form, form.startDate, time))}
							oninvalid={(bad) => (unreadable.startTime = bad)}
						/>
					{/if}
				</div>
			</div>

			<div class="row">
				<span class="label" id="editor-ends">Ends</span>
				<div class="when">
					<DateField
						labelledby="editor-ends"
						class="date tabular"
						value={form.endDate}
						onvalue={(date) => (form.endDate = date)}
						oninvalid={(bad) => (unreadable.endDate = bad)}
					/>
					{#if !form.allDay}
						<TimeField
							label="End time"
							start={form.endDate === form.startDate ? form.startTime : null}
							class="time tabular"
							value={form.endTime}
							onvalue={(time) => (form.endTime = time)}
							oninvalid={(bad) => (unreadable.endTime = bad)}
						/>
					{/if}
				</div>
			</div>

			{#if !form.allDay}
				<label class="row">
					<span class="label">Time zone</span>
					<select bind:value={form.timeZone}>
						{#each zones as zone (zone)}
							<option value={zone}>{zoneLabel(zone)}</option>
						{/each}
					</select>
				</label>
			{/if}

			<label class="row">
				<span class="label">Repeat</span>
				<select bind:value={form.repeat}>
					<option value="never">Never</option>
					<option value="daily">Every day</option>
					<option value="weekly">Every week</option>
					<option value="monthly">Every month</option>
					<option value="yearly">Every year</option>
					{#if initial.repeat === 'custom'}
						<option value="custom">Custom</option>
					{/if}
				</select>
			</label>

			{#if repeats}
				<div class="row">
					<span class="label" id="editor-end-repeat">End repeat</span>
					<div class="when">
						<select
							class="end-choice"
							aria-labelledby="editor-end-repeat"
							value={form.repeatUntil === null ? 'never' : 'on'}
							onchange={(e) => (form = withRepeatEnd(form, e.currentTarget.value === 'on'))}
						>
							<option value="never">Never</option>
							<option value="on">On date</option>
						</select>
						{#if form.repeatUntil !== null}
							<DateField
								labelledby="editor-end-repeat"
								class="date tabular"
								value={form.repeatUntil}
								onvalue={(date) => (form.repeatUntil = date)}
								oninvalid={(bad) => (unreadable.repeatUntil = bad)}
							/>
						{/if}
					</div>
				</div>
			{/if}
		</div>

		<div class="rows">
			<label class="row">
				<span class="label">Calendar</span>
				<span class="calendar">
					<span class="dot" aria-hidden="true"></span>
					<select bind:value={form.calendarId}>
						{#each writable as option (option.id)}
							<option value={option.id}>{option.name}</option>
						{:else}
							<option value={NEW_LOCAL_CALENDAR}>Personal</option>
						{/each}
					</select>
				</span>
			</label>

			<div class="row top">
				<span class="label" id="editor-reminders">Reminders</span>
				<div class="reminders" role="group" aria-labelledby="editor-reminders">
					{#each form.reminders as reminder, i (i)}
						<div class="reminder">
							<select
								aria-label="Reminder {i + 1}"
								value={reminder.minutes_before}
								onchange={(e) => setReminder(i, Number(e.currentTarget.value))}
							>
								{#each choicesFor(reminder.minutes_before) as minutes (minutes)}
									<option value={minutes}>{reminderLabel(minutes)}</option>
								{/each}
							</select>
							{#if reminder.method === 'email'}
								<span class="note">by email</span>
							{/if}
							<button
								type="button"
								class="icon-button"
								aria-label="Remove reminder {i + 1}"
								title="Remove"
								onclick={() => removeReminder(i)}
							>
								<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
									<path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
								</svg>
							</button>
						</div>
					{/each}
					{#if form.useDefaultReminders && form.reminders.length > 0}
						<span class="note">The calendar’s default</span>
					{/if}
					<button type="button" class="button quiet add" onclick={addReminder}>
						Add reminder
					</button>
				</div>
			</div>
		</div>

		<textarea
			bind:value={form.notes}
			class="notes"
			rows="3"
			placeholder="Add notes"
			aria-label="Notes"></textarea>

		{#if error ?? problem}
			<p class="error" role="alert">{error ?? problem}</p>
		{/if}

		{#if asking}
			<footer class="asking" role="group" aria-labelledby="editor-scope">
				<p id="editor-scope" class="question">
					{asking.includes('this')
						? 'This is a repeating event. Save it for this event only, or for all events?'
						: 'This change applies to all events in the series.'}
				</p>
				<button type="button" class="button quiet" onclick={() => (asking = null)}>Cancel</button>
				{#if asking.includes('this')}
					<button type="button" class="button" disabled={saving} onclick={() => save('this')}>
						This event
					</button>
				{/if}
				<button type="button" class="button primary" disabled={saving} onclick={() => save('all')}>
					All events
				</button>
			</footer>
		{:else}
			<footer>
				<button type="button" class="button quiet" onclick={() => dialog?.close()}>Cancel</button>
				<button type="submit" class="button primary" disabled={saving || problem !== null}>
					{saving ? 'Saving…' : isNew ? 'Add' : 'Save'}
				</button>
			</footer>
		{/if}
	</form>
</dialog>

<style>
	.dialog {
		width: min(440px, calc(100vw - 2 * var(--space-xl)));
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

	form {
		display: flex;
		flex-direction: column;
	}

	header {
		margin-bottom: var(--space-sm);
	}

	.heading {
		margin: 0;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
		color: var(--text-secondary);
	}

	p {
		margin: 0;
	}

	/* Title and location: plain text on the dialog, no box, like a document */
	.title,
	.location {
		width: 100%;
		padding: var(--space-2xs) 0;
		border: 0;
		background: transparent;
		-webkit-user-select: text;
		user-select: text;
	}

	.title {
		font-size: var(--text-headline);
		line-height: var(--leading-headline);
		font-weight: var(--weight-semibold);
	}

	.location {
		color: var(--text-secondary);
	}

	.title::placeholder,
	.location::placeholder,
	.notes::placeholder {
		color: var(--text-tertiary);
	}

	.title:focus-visible,
	.location:focus-visible {
		outline: none;
	}

	/* Groups of label-and-control rows, separated by hairlines */
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--space-sm);
		margin-top: var(--space-md);
		padding-top: var(--space-md);
		border-top: 1px solid var(--separator);
	}

	.row {
		display: grid;
		grid-template-columns: 88px minmax(0, 1fr);
		align-items: center;
		gap: var(--space-sm);
		min-height: var(--control-height);
	}

	.row.top {
		align-items: start;
	}

	.row.top .label {
		line-height: var(--control-height);
	}

	.label {
		color: var(--text-secondary);
	}

	.when {
		display: flex;
		gap: var(--space-xs);
	}

	.when :global(input),
	select {
		height: var(--control-height);
		padding: 0 var(--space-sm);
		border: 1px solid var(--separator);
		border-radius: var(--radius-md);
		background: var(--surface);
		color: var(--text);
		min-width: 0;
	}

	.when :global(.date) {
		flex: 1 1 120px;
	}

	.when :global(.time) {
		flex: 0 0 80px;
	}

	.when .end-choice {
		flex: 1 1 100px;
		width: auto;
	}

	.when :global(input) {
		-webkit-user-select: text;
		user-select: text;
	}

	select {
		width: 100%;
		text-overflow: ellipsis;
	}

	/* All-day switch: the track is the checkbox, the knob its ::before */
	.switch {
		appearance: none;
		position: relative;
		justify-self: start;
		width: 32px;
		height: 18px;
		margin: 0;
		border-radius: var(--radius-full);
		background: var(--fill-control);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.switch::before {
		content: '';
		position: absolute;
		top: 2px;
		left: 2px;
		width: 14px;
		height: 14px;
		border-radius: var(--radius-full);
		background: var(--control-thumb);
		box-shadow: var(--shadow-1);
		transition: transform var(--duration-fast) var(--ease-standard);
	}

	.switch:checked {
		background: var(--accent);
	}

	.switch:checked::before {
		background: var(--on-accent);
		transform: translateX(14px);
	}

	.calendar {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		min-width: 0;
	}

	.dot {
		flex: none;
		width: 10px;
		height: 10px;
		border-radius: var(--radius-full);
		background: var(--cal);
	}

	.reminders {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--space-xs);
		min-width: 0;
	}

	.reminder {
		display: flex;
		align-items: center;
		gap: var(--space-xs);
		width: 100%;
	}

	.reminder select {
		flex: 1;
	}

	.note {
		flex: none;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		color: var(--text-secondary);
	}

	.add {
		margin-left: calc(-1 * var(--space-md));
	}

	.notes {
		margin-top: var(--space-md);
		padding: var(--space-md) 0 0;
		border: 0;
		border-top: 1px solid var(--separator);
		border-radius: 0;
		background: transparent;
		resize: vertical;
		-webkit-user-select: text;
		user-select: text;
	}

	.notes:focus-visible {
		outline: none;
	}

	.error {
		margin-top: var(--space-md);
		color: var(--danger);
	}

	footer {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--space-xs);
		margin-top: var(--space-lg);
	}

	/* The question takes its own line above the answers */
	.question {
		flex-basis: 100%;
		margin-bottom: var(--space-xs);
		color: var(--text-secondary);
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

	.button.primary {
		border-color: transparent;
		color: var(--on-accent);
		background: var(--accent);
	}

	.icon-button {
		flex: none;
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
