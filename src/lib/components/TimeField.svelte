<script lang="ts">
	// A time field with a menu of quarter hours, as in Google Calendar: focusing it opens a
	// scrollable list at the current time, and any other time can still be typed (`930`,
	// `21h30`). Always a 24-hour clock (`display-format.ts`), whatever the system locale.
	import { durationLabel, parseTimeField, timeOptions } from '#lib/display-format.js';
	import { tick, untrack } from 'svelte';

	interface Props {
		/** `HH:MM` */
		value: string;
		/** A new value, as soon as what is typed can be read, or an option is chosen */
		onvalue: (value: string) => void;
		/** Whether what is typed cannot be read */
		oninvalid: (invalid: boolean) => void;
		label: string;
		/** For an end time on the start's day: the menu starts here and shows each length */
		start?: string | null;
		class?: string;
	}

	let { value, onvalue, oninvalid, label, start = null, class: className }: Props = $props();

	const id = $props.id();
	const options = $derived(timeOptions(start));
	const minutes = (time: string) => Number(time.slice(0, 2)) * 60 + Number(time.slice(3, 5));

	// svelte-ignore state_referenced_locally
	let text = $state(value);
	let focused = $state(false);
	let open = $state(false);
	/** The highlighted option */
	let active = $state(-1);
	let list: HTMLElement | undefined = $state();

	// As in `DateField`: a value changed from outside shows at once, typed text stays
	$effect(() => {
		const shown = value;
		untrack(() => {
			if (focused) return;
			text = shown;
			oninvalid(false);
		});
	});

	/** The option at `time`, else the first one after it */
	function nearest(time: string): number {
		const index = options.findIndex((option) => minutes(option) >= minutes(time));
		return index === -1 ? options.length - 1 : index;
	}

	async function show() {
		if (open) return;
		open = true;
		active = nearest(parseTimeField(text) ?? value);
		await tick();
		reveal('center');
	}

	function reveal(block: ScrollLogicalPosition) {
		list?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block });
	}

	function choose(time: string) {
		text = time;
		open = false;
		oninvalid(false);
		if (time !== value) onvalue(time);
	}

	function oninput() {
		const parsed = parseTimeField(text);
		oninvalid(parsed === null);
		if (parsed === null) return;
		if (parsed !== value) onvalue(parsed);
		active = nearest(parsed);
		void tick().then(() => reveal('nearest'));
	}

	function onblur() {
		focused = false;
		open = false;
		const parsed = parseTimeField(text);
		if (parsed !== null) text = parsed;
	}

	async function onkeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			if (!open) return show();
			const step = e.key === 'ArrowDown' ? 1 : -1;
			active = Math.min(options.length - 1, Math.max(0, active + step));
			await tick();
			reveal('nearest');
		} else if (e.key === 'Enter' && open && active >= 0) {
			// Choose the option instead of submitting the form
			e.preventDefault();
			choose(options[active]);
		} else if (e.key === 'Escape' && open) {
			// Close the menu, not the dialog
			e.preventDefault();
			e.stopPropagation();
			open = false;
		} else if (e.key === 'Tab') {
			open = false;
		}
	}
</script>

<div class={['time-field', className]}>
	<input
		type="text"
		role="combobox"
		aria-label={label}
		aria-expanded={open}
		aria-controls="{id}-list"
		aria-autocomplete="list"
		aria-activedescendant={open && active >= 0 ? `${id}-${active}` : undefined}
		bind:value={text}
		placeholder="HH:MM"
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
		<!-- Pressing an option must not take the focus from the field, or it would close first -->
		<ul
			bind:this={list}
			id="{id}-list"
			class="menu"
			role="listbox"
			aria-label={label}
			tabindex="-1"
			onpointerdown={(e) => e.preventDefault()}
		>
			{#each options as option, i (option)}
				<!-- The keyboard works through the field (arrows, Enter), as for any combobox -->
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<li
					id="{id}-{i}"
					data-index={i}
					role="option"
					aria-selected={option === value}
					class={{ option: true, active: i === active, selected: option === value }}
					onclick={() => choose(option)}
					onpointermove={() => (active = i)}
				>
					<span class="tabular">{option}</span>
					{#if start !== null}
						<span class="length">{durationLabel(minutes(option) - minutes(start))}</span>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.time-field {
		position: relative;
		display: flex;
		/* A text input is about 20 characters wide by itself; the parent decides the width */
		min-width: 0;
	}

	input {
		flex: 1;
		min-width: 0;
	}

	.menu {
		position: absolute;
		top: calc(100% + var(--space-xs));
		/* Hangs from the right edge: the time sits at the right of its row, and a menu wider
		   than the field must not stick out of the dialog */
		right: 0;
		z-index: 5;
		min-width: max(100%, 168px);
		max-height: 224px;
		margin: 0;
		padding: var(--space-xs);
		list-style: none;
		overflow-y: auto;
		border-radius: var(--radius-md);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-2);
		animation: menu-in var(--duration-fast) var(--ease-standard) both;
		transform-origin: right top;
	}

	@keyframes menu-in {
		from {
			opacity: 0;
			transform: scale(0.96);
		}
		to {
			opacity: 1;
			transform: none;
		}
	}

	.option {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-md);
		height: var(--control-height);
		padding: 0 var(--space-sm);
		border-radius: var(--radius-sm);
		white-space: nowrap;
	}

	.option.active {
		background: var(--fill-hover);
	}

	.option.selected {
		color: var(--accent-text);
		font-weight: var(--weight-medium);
	}

	.length {
		font-size: var(--text-footnote);
		color: var(--text-secondary);
	}
</style>
