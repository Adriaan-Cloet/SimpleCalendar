<script lang="ts">
	import type { CalendarInfo, CalendarUpdate } from '#lib/calendar.js';
	import { calendarColor } from '#lib/calendar-color.js';
	import { groupCalendars } from '#lib/calendar-groups.js';
	import CalendarColorMenu from '#lib/components/CalendarColorMenu.svelte';
	import { serviceError } from '#lib/sources.js';
	import { addDays, isSameDay, startOfWeek } from '#lib/week-dates.js';

	interface Props {
		today: Date;
		/** The Monday of the week on screen; its row in the mini month is marked */
		weekStart: Date;
		calendars: CalendarInfo[];
		/** Shows or hides a calendar, or changes its colour */
		onupdate: (id: number, update: CalendarUpdate) => void;
		/** Opens the subscriptions dialog */
		onsubscriptions: () => void;
		/** A new calendar on this computer; rejects with a `ServiceError` */
		oncreatecalendar: (name: string) => Promise<void>;
		/** Deletes a local calendar with its events; rejects with a `ServiceError` */
		onremovecalendar: (id: number) => Promise<void>;
	}

	let {
		today,
		weekStart,
		calendars,
		onupdate,
		onsubscriptions,
		oncreatecalendar,
		onremovecalendar
	}: Props = $props();

	const monthTitle = $derived(
		new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).format(today)
	);

	// Whole weeks covering the current month, Monday first
	const weeks = $derived.by(() => {
		const first = new Date(today.getFullYear(), today.getMonth(), 1);
		const rows: Date[][] = [];
		let cursor = startOfWeek(first);
		while (rows.length === 0 || cursor.getMonth() === today.getMonth()) {
			rows.push(Array.from({ length: 7 }, (_, i) => addDays(cursor, i)));
			cursor = addDays(cursor, 7);
		}
		return rows;
	});

	const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: 'narrow' });
	const weekdays = $derived(weeks[0].map((day) => weekdayFormat.format(day)));
	const dayLabel = new Intl.DateTimeFormat(undefined, { dateStyle: 'full' });

	const groups = $derived(groupCalendars(calendars));

	// The colour menu, opened with a right click or the context menu key on a calendar
	let colorMenu = $state<{ id: number; x: number; y: number; row: HTMLElement } | null>(null);
	const colorMenuCalendar = $derived(calendars.find((c) => c.id === colorMenu?.id));

	function oncontextmenu(e: MouseEvent, id: number) {
		e.preventDefault();
		const row = e.currentTarget as HTMLElement;
		// From the keyboard (no right button) there is no pointer position: open below the row
		const rect = row.getBoundingClientRect();
		colorMenu =
			e.button !== 2
				? { id, x: rect.left, y: rect.bottom, row }
				: { id, x: e.clientX, y: e.clientY, row };
	}

	// Local calendars (#15): the one being renamed, a new one being named, and the one whose
	// deletion waits for confirmation
	let renaming = $state<number | null>(null);
	let adding = $state(false);
	let deleting = $state<number | null>(null);
	let calendarError = $state<string | null>(null);

	function rename(calendar: CalendarInfo, name: string) {
		renaming = null;
		if (name.trim() && name.trim() !== calendar.name) onupdate(calendar.id, { name });
	}

	async function add(name: string) {
		adding = false;
		if (!name.trim()) return;
		try {
			await oncreatecalendar(name);
			calendarError = null;
		} catch (e) {
			calendarError = serviceError(e).message;
		}
	}

	async function remove(id: number) {
		deleting = null;
		try {
			await onremovecalendar(id);
			calendarError = null;
		} catch (e) {
			calendarError = serviceError(e).message;
		}
	}

	/** Enter commits the name, Escape gives up; leaving the field commits as well. `done`
	 * marks the field, so a blur while it is removed does not commit a second time. */
	function nameKeys(e: KeyboardEvent, commit: (name: string) => void, cancel: () => void) {
		const input = e.currentTarget as HTMLInputElement;
		if (e.key !== 'Enter' && e.key !== 'Escape') return;
		e.stopPropagation();
		input.dataset.done = 'true';
		if (e.key === 'Enter') commit(input.value);
		else cancel();
	}

	function closeColorMenu(restoreFocus: boolean) {
		if (restoreFocus) colorMenu?.row.querySelector('input')?.focus();
		colorMenu = null;
	}

	// Prototype only: in the app this becomes a setting. No attribute means "follow the system".
	type Appearance = 'system' | 'light' | 'dark';
	const appearances: { value: Appearance; label: string }[] = [
		{ value: 'system', label: 'System' },
		{ value: 'light', label: 'Light' },
		{ value: 'dark', label: 'Dark' }
	];
	let appearance = $state<Appearance>('system');

	$effect(() => {
		const root = document.documentElement;
		if (appearance === 'system') delete root.dataset.theme;
		else root.dataset.theme = appearance;
	});
</script>

<aside class="sidebar">
	<section class="mini-month" aria-label="Month overview">
		<div class="mini-header">
			<span class="mini-title">{monthTitle}</span>
		</div>
		<div class="mini-grid tabular" role="grid">
			<div class="mini-row" role="row">
				{#each weekdays as weekday, i (i)}
					<span class="mini-weekday" role="columnheader">{weekday}</span>
				{/each}
			</div>
			{#each weeks as week (week[0].getTime())}
				<div class={{ 'mini-row': true, 'current-week': isSameDay(week[0], weekStart) }} role="row">
					{#each week as day (day.getTime())}
						<span
							role="gridcell"
							aria-label={dayLabel.format(day)}
							class={{
								'mini-day': true,
								outside: day.getMonth() !== today.getMonth(),
								today: isSameDay(day, today)
							}}><span class="num">{day.getDate()}</span></span
						>
					{/each}
				</div>
			{/each}
		</div>
	</section>

	<nav class="calendars" aria-label="Calendars">
		{#each groups as group (group.key)}
			<section class="group">
				<div class="group-header">
					<h2 class="group-title">{group.label}</h2>
					{#if group.key === 'local'}
						<button
							type="button"
							class="group-add"
							aria-label="New calendar"
							title="New calendar"
							onclick={() => (adding = true)}
						>
							<svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
								<path d="M8 3.5v9M3.5 8h9" />
							</svg>
						</button>
					{/if}
				</div>
				<ul>
					{#each group.calendars as calendar (calendar.id)}
						<li>
							{#if renaming === calendar.id}
								<div
									class="calendar editing"
									style:--cal={calendarColor(calendar.color, calendar.id)}
								>
									<span class="swatch" aria-hidden="true"></span>
									<input
										class="name-input"
										type="text"
										value={calendar.name}
										aria-label="Name of {calendar.name}"
										{@attach (input) => input.select()}
										onkeydown={(e) =>
											nameKeys(
												e,
												(name) => rename(calendar, name),
												() => (renaming = null)
											)}
										onblur={(e) => {
											if (!e.currentTarget.dataset.done) rename(calendar, e.currentTarget.value);
										}}
									/>
								</div>
							{:else}
								<label
									class={{ calendar: true, 'menu-open': colorMenu?.id === calendar.id }}
									style:--cal={calendarColor(calendar.color, calendar.id)}
									title="Right-click to change the colour"
									oncontextmenu={(e) => oncontextmenu(e, calendar.id)}
								>
									<input
										type="checkbox"
										checked={calendar.visible}
										onchange={(e) => onupdate(calendar.id, { visible: e.currentTarget.checked })}
									/>
									<span class="calendar-name">{calendar.name}</span>
									{#if !calendar.editable}
										<svg
											class="lock"
											viewBox="0 0 16 16"
											width="12"
											height="12"
											aria-label="Read-only"
											role="img"
										>
											<rect x="3.5" y="7" width="9" height="6.5" rx="1.5" />
											<path d="M5.5 7V5a2.5 2.5 0 0 1 5 0v2" />
										</svg>
									{/if}
								</label>
							{/if}
							{#if deleting === calendar.id}
								<div class="confirm" role="alert">
									<p>Delete “{calendar.name}” and all its events?</p>
									<div class="confirm-actions">
										<button type="button" class="small quiet" onclick={() => (deleting = null)}>
											Cancel
										</button>
										<button type="button" class="small danger" onclick={() => remove(calendar.id)}>
											Delete
										</button>
									</div>
								</div>
							{/if}
						</li>
					{/each}
					{#if adding && group.key === 'local'}
						<li>
							<div class="calendar editing">
								<span class="swatch empty" aria-hidden="true"></span>
								<input
									class="name-input"
									type="text"
									placeholder="Calendar name"
									aria-label="Name of the new calendar"
									{@attach (input) => input.focus()}
									onkeydown={(e) => nameKeys(e, add, () => (adding = false))}
									onblur={(e) => {
										if (!e.currentTarget.dataset.done) add(e.currentTarget.value);
									}}
								/>
							</div>
						</li>
					{/if}
				</ul>
				{#if calendarError && group.key === 'local'}
					<p class="calendar-error" role="alert">{calendarError}</p>
				{/if}
			</section>
		{/each}
	</nav>

	<footer class="footer">
		<button type="button" class="footer-button" onclick={onsubscriptions}>
			<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
				<path d="M3.5 3.5a9 9 0 0 1 9 9M3.5 7.5a5 5 0 0 1 5 5" />
				<circle class="filled" cx="4.25" cy="11.75" r="1.25" />
			</svg>
			Accounts
		</button>
		<div class="appearance segmented" role="radiogroup" aria-label="Appearance">
			{#each appearances as option (option.value)}
				<button
					type="button"
					role="radio"
					aria-checked={appearance === option.value}
					aria-label={option.label}
					title={option.label}
					class={{ segment: true, selected: appearance === option.value }}
					onclick={() => (appearance = option.value)}
				>
					<svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
						{#if option.value === 'system'}
							<!-- Half-filled circle: follows the system -->
							<circle cx="8" cy="8" r="5.5" />
							<path class="filled" d="M8 2.5a5.5 5.5 0 0 0 0 11z" />
						{:else if option.value === 'light'}
							<circle cx="8" cy="8" r="2.75" />
							<path
								d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1.06 1.06M11.54 11.54l1.06 1.06M3.4 12.6l1.06-1.06M11.54 4.46l1.06-1.06"
							/>
						{:else}
							<path d="M13.5 9.6A5.75 5.75 0 0 1 6.4 2.5a5.75 5.75 0 1 0 7.1 7.1z" />
						{/if}
					</svg>
				</button>
			{/each}
		</div>
	</footer>
</aside>

{#if colorMenu && colorMenuCalendar}
	<CalendarColorMenu
		calendar={colorMenuCalendar}
		x={colorMenu.x}
		y={colorMenu.y}
		onpick={(color) => onupdate(colorMenuCalendar.id, { color_override: color })}
		onclose={closeColorMenu}
		onrename={colorMenuCalendar.source_kind === 'local'
			? () => (renaming = colorMenuCalendar.id)
			: undefined}
		ondelete={colorMenuCalendar.source_kind === 'local'
			? () => (deleting = colorMenuCalendar.id)
			: undefined}
	/>
{/if}

<style>
	.sidebar {
		display: flex;
		flex-direction: column;
		gap: var(--space-xl);
		width: var(--sidebar-width);
		height: 100%;
		padding: var(--space-lg) var(--space-md);
		background: var(--surface-sidebar);
		border-right: 1px solid var(--separator);
		overflow-y: auto;
	}

	/* Narrow windows give the week all the room. The app will get a toggle for this. */
	@media (max-width: 899px) {
		.sidebar {
			display: none;
		}
	}

	/* Mini month */
	.mini-header {
		padding: 0 var(--space-xs) var(--space-sm);
	}

	.mini-title {
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
	}

	.mini-row {
		display: grid;
		grid-template-columns: repeat(7, 1fr);
		border-radius: var(--radius-md);
	}

	.mini-row.current-week {
		background: var(--fill-pressed);
	}

	.mini-weekday,
	.mini-day {
		display: grid;
		place-items: center;
		height: 26px;
		font-size: var(--text-caption);
		line-height: var(--leading-caption);
	}

	.mini-weekday {
		color: var(--text-secondary);
		font-weight: var(--weight-medium);
	}

	.mini-day.outside {
		color: var(--text-tertiary);
	}

	.num {
		display: grid;
		place-items: center;
		width: 22px;
		height: 22px;
		border-radius: var(--radius-full);
	}

	.mini-day.today .num {
		color: var(--on-now);
		font-weight: var(--weight-semibold);
		background: var(--now);
	}

	/* Calendar list */
	.calendars {
		display: flex;
		flex-direction: column;
		gap: var(--space-lg);
		flex: 1;
	}

	.group-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin: 0 0 var(--space-xs);
	}

	.group-add {
		display: grid;
		place-items: center;
		width: 20px;
		height: 20px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.group-add:hover {
		color: var(--text);
		background: var(--fill-hover);
	}

	.group-add svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.6;
		stroke-linecap: round;
	}

	/* Naming a local calendar: the row keeps its height, the name becomes a field */
	.calendar.editing {
		background: var(--fill-hover);
	}

	.swatch {
		flex: none;
		width: 14px;
		height: 14px;
		border-radius: var(--radius-sm);
		background: var(--cal);
	}

	.swatch.empty {
		background: transparent;
		border: 1.5px dashed var(--text-tertiary);
	}

	.name-input {
		flex: 1;
		min-width: 0;
		height: 22px;
		padding: 0 var(--space-xs);
		border: 1px solid var(--separator);
		border-radius: var(--radius-sm);
		background: var(--surface);
		-webkit-user-select: text;
		user-select: text;
	}

	.name-input:focus-visible {
		outline-offset: 0;
	}

	.confirm {
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		margin: var(--space-2xs) 0 var(--space-xs);
		padding: var(--space-sm);
		border-radius: var(--radius-md);
		background: var(--fill-hover);
	}

	.confirm p,
	.calendar-error {
		margin: 0;
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
	}

	.calendar-error {
		padding: var(--space-xs);
		color: var(--danger);
	}

	.confirm-actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--space-xs);
	}

	.small {
		height: 24px;
		padding: 0 var(--space-sm);
		border: 0;
		border-radius: var(--radius-md);
		background: transparent;
		font-size: var(--text-footnote);
		font-weight: var(--weight-medium);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.small.quiet {
		color: var(--text-secondary);
	}

	.small.danger {
		color: var(--danger);
	}

	.small:hover {
		background: var(--fill-pressed);
	}

	.group-title {
		margin: 0;
		padding: 0 var(--space-xs);
		font-size: var(--text-footnote);
		line-height: var(--leading-footnote);
		font-weight: var(--weight-semibold);
		color: var(--text-secondary);
	}

	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.calendar {
		display: flex;
		align-items: center;
		gap: var(--space-sm);
		height: var(--control-height);
		padding: 0 var(--space-xs);
		border-radius: var(--radius-md);
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.calendar:hover,
	.calendar.menu-open {
		background: var(--fill-hover);
	}

	.calendar-name {
		flex: 1;
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	input[type='checkbox'] {
		appearance: none;
		flex: none;
		width: 14px;
		height: 14px;
		margin: 0;
		border: 1.5px solid var(--cal);
		border-radius: var(--radius-sm);
		background: transparent no-repeat center / 10px 10px;
		transition:
			background-color var(--duration-fast) var(--ease-standard),
			border-color var(--duration-fast) var(--ease-standard);
	}

	input[type='checkbox']:checked {
		background-color: var(--cal);
		/* White check mark */
		background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'%3E%3Cpath d='M2 5.2 4.1 7.3 8 2.8' fill='none' stroke='white' stroke-width='1.6' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E");
	}

	.lock {
		flex: none;
		fill: none;
		stroke: var(--text-tertiary);
		stroke-width: 1.4;
		stroke-linecap: round;
	}

	/* Footer: subscriptions and appearance */
	.footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-sm);
	}

	.footer-button {
		display: inline-flex;
		align-items: center;
		gap: var(--space-xs);
		min-width: 0;
		height: var(--control-height);
		padding: 0 var(--space-xs);
		border: 0;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text-secondary);
		white-space: nowrap;
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.footer-button:hover {
		color: var(--text);
		background: var(--fill-hover);
	}

	.footer-button:active {
		background: var(--fill-pressed);
	}

	.footer-button svg {
		flex: none;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
	}

	.footer-button .filled {
		fill: currentColor;
		stroke: none;
	}

	/* Appearance control (prototype only): icons only, sized to its content */
	.segmented {
		display: flex;
		flex: none;
		padding: var(--space-2xs);
		border-radius: var(--radius-md);
		background: var(--fill-control);
	}

	.segment {
		display: grid;
		place-items: center;
		width: 28px;
		height: 22px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		transition:
			background-color var(--duration-fast) var(--ease-standard),
			color var(--duration-fast) var(--ease-standard);
	}

	.segment:hover {
		color: var(--text);
	}

	.segment.selected {
		color: var(--text);
		background: var(--control-thumb);
		box-shadow: var(--shadow-1);
	}

	.segment svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.segment .filled {
		fill: currentColor;
		stroke: none;
	}
</style>
