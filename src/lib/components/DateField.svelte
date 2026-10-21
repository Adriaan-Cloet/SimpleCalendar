<script lang="ts">
	// A date field in the app's own format (`display-format.ts`), `20/10/2026`, with a small
	// calendar to pick from. The native date input always follows the system locale, so on an
	// English system it shows month-first dates. A date can also be typed (`20-10-26`, `20/10`).
	import { displayLocale, formatDateField, parseDateField } from '#lib/display-format.js';
	import { shiftDate } from '#lib/event-form.js';
	import { isoDate, isSameDay, monthWeeks } from '#lib/week-dates.js';
	import { untrack } from 'svelte';

	interface Props {
		/** `YYYY-MM-DD` */
		value: string;
		/** A new value, as soon as what is typed can be read, or a day is picked */
		onvalue: (value: string) => void;
		/** Whether what is typed cannot be read */
		oninvalid: (invalid: boolean) => void;
		labelledby: string;
		class?: string;
	}

	let { value, onvalue, oninvalid, labelledby, class: className }: Props = $props();

	const id = $props.id();

	// A date typed without a year is in the year of the current value
	const parse = (text: string) => parseDateField(text, Number(value.slice(0, 4)));
	// svelte-ignore state_referenced_locally
	let text = $state(formatDateField(value));
	let focused = $state(false);
	let open = $state(false);
	/** The month the calendar shows: its first day */
	// svelte-ignore state_referenced_locally
	let month = $state(firstOfMonth(value));

	const weeks = $derived(monthWeeks(month.getFullYear(), month.getMonth(), 6));
	const today = new Date();
	const monthTitle = $derived(
		new Intl.DateTimeFormat(displayLocale, { month: 'long', year: 'numeric' }).format(month)
	);
	const weekdayFormat = new Intl.DateTimeFormat(displayLocale, { weekday: 'narrow' });
	const dayLabel = new Intl.DateTimeFormat(displayLocale, { dateStyle: 'full' });

	function firstOfMonth(date: string): Date {
		return new Date(Number(date.slice(0, 4)), Number(date.slice(5, 7)) - 1, 1);
	}

	// A value changed from outside, such as the end moving with the start, shows at once; while
	// typing, the text stays as typed. Only `value` triggers this: leaving a field with
	// unreadable text keeps that text, next to the error, instead of quietly undoing it.
	$effect(() => {
		const shown = formatDateField(value);
		untrack(() => {
			if (focused) return;
			text = shown;
			oninvalid(false);
		});
	});

	function show() {
		if (open) return;
		month = firstOfMonth(parse(text) ?? value);
		open = true;
	}

	function set(date: string) {
		text = formatDateField(date);
		oninvalid(false);
		if (date !== value) onvalue(date);
		month = firstOfMonth(date);
	}

	function pick(day: Date) {
		set(isoDate(day));
		open = false;
	}

	function oninput() {
		const parsed = parse(text);
		oninvalid(parsed === null);
		if (parsed === null) return;
		if (parsed !== value) onvalue(parsed);
		month = firstOfMonth(parsed);
	}

	function onblur() {
		focused = false;
		open = false;
		const parsed = parse(text);
		if (parsed !== null) text = formatDateField(parsed);
	}

	// While the calendar is open, up and down move a week through its grid; while it is closed,
	// up and down step a day later or earlier, as a native field would
	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
			e.preventDefault();
			const up = e.key === 'ArrowUp';
			const days = open ? (up ? -7 : 7) : up ? 1 : -1;
			set(shiftDate(parse(text) ?? value, days));
		} else if ((e.key === 'Escape' || e.key === 'Enter') && open) {
			// Close the calendar, not the dialog, and do not submit yet
			e.preventDefault();
			e.stopPropagation();
			open = false;
		} else if (e.key === 'Tab') {
			open = false;
		}
	}

	function moveMonth(step: number) {
		month = new Date(month.getFullYear(), month.getMonth() + step, 1);
	}
</script>

<div class={['date-field', className]}>
	<input
		type="text"
		role="combobox"
		aria-labelledby={labelledby}
		aria-haspopup="dialog"
		aria-expanded={open}
		aria-controls="{id}-calendar"
		bind:value={text}
		placeholder="DD/MM/YYYY"
		inputmode="numeric"
		autocomplete="off"
		spellcheck="false"
		onfocus={() => {
			focused = true;
			show();
		}}
		onclick={show}
		{onblur}
		{oninput}
		{onkeydown}
	/>
	{#if open}
		<!-- Pressing inside must not take the focus from the field, or it would close first. The
		     keyboard works through the field: arrows step days and weeks. -->
		<div
			id="{id}-calendar"
			class="calendar"
			role="group"
			aria-label="Choose a date"
			onpointerdown={(e) => e.preventDefault()}
		>
			<div class="header">
				<span class="title">{monthTitle}</span>
				<button
					type="button"
					class="nav"
					tabindex="-1"
					aria-label="Previous month"
					onclick={() => moveMonth(-1)}
				>
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
						<path d="M10 3.5 5.5 8l4.5 4.5" />
					</svg>
				</button>
				<button
					type="button"
					class="nav"
					tabindex="-1"
					aria-label="Next month"
					onclick={() => moveMonth(1)}
				>
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
						<path d="M6 3.5 10.5 8 6 12.5" />
					</svg>
				</button>
			</div>
			<div class="grid tabular">
				{#each weeks[0] as day (day.getTime())}
					<span class="weekday" aria-hidden="true">{weekdayFormat.format(day)}</span>
				{/each}
				{#each weeks.flat() as day (day.getTime())}
					<button
						type="button"
						tabindex="-1"
						aria-label={dayLabel.format(day)}
						aria-pressed={isoDate(day) === value}
						class={{
							day: true,
							outside: day.getMonth() !== month.getMonth(),
							today: isSameDay(day, today),
							selected: isoDate(day) === value
						}}
						onclick={() => pick(day)}>{day.getDate()}</button
					>
				{/each}
			</div>
		</div>
	{/if}
</div>

<style>
	.date-field {
		position: relative;
		display: flex;
		/* A text input is about 20 characters wide by itself; the parent decides the width */
		min-width: 0;
	}

	input {
		flex: 1;
		min-width: 0;
	}

	.calendar {
		position: absolute;
		top: calc(100% + var(--space-xs));
		left: 0;
		z-index: 5;
		width: 248px;
		padding: var(--space-sm);
		border-radius: var(--radius-md);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-2);
		animation: calendar-in var(--duration-fast) var(--ease-standard) both;
		transform-origin: left top;
	}

	@keyframes calendar-in {
		from {
			opacity: 0;
			transform: scale(0.96);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	.header {
		display: flex;
		align-items: center;
		gap: var(--space-2xs);
		padding: 0 0 var(--space-xs) var(--space-xs);
	}

	.title {
		flex: 1;
		font-weight: var(--weight-semibold);
	}

	.nav {
		display: grid;
		place-items: center;
		width: 24px;
		height: 24px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.nav:hover {
		color: var(--text);
		background: var(--fill-hover);
	}

	.nav svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.grid {
		display: grid;
		grid-template-columns: repeat(7, 1fr);
		gap: var(--space-2xs);
	}

	.weekday {
		display: grid;
		place-items: center;
		height: 24px;
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
		font-weight: var(--weight-medium);
		color: var(--text-secondary);
	}

	.day {
		display: grid;
		place-items: center;
		height: 30px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-full);
		background: transparent;
		font-size: var(--text-footnote);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.day:hover {
		background: var(--fill-hover);
	}

	.day.outside {
		color: var(--text-tertiary);
	}

	.day.today {
		color: var(--now-text);
		font-weight: var(--weight-semibold);
	}

	.day.selected {
		color: var(--on-accent);
		background: var(--accent);
		font-weight: var(--weight-semibold);
	}
</style>
