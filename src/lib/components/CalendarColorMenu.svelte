<script lang="ts">
	import type { CalendarInfo } from '#lib/calendar.js';
	import { palette } from '#lib/calendar-color.js';

	interface Props {
		calendar: CalendarInfo;
		/** Where it opens: the pointer, or the row when opened from the keyboard */
		x: number;
		y: number;
		/** A palette `#rrggbb` value, or `null` for the provider's colour */
		onpick: (color: string | null) => void;
		/** `restoreFocus` is true when it closed from the keyboard or a pick */
		onclose: (restoreFocus: boolean) => void;
		/** Offered for local calendars only */
		onrename?: () => void;
		ondelete?: () => void;
	}

	let { calendar, x, y, onpick, onclose, onrename, ondelete }: Props = $props();

	let menu: HTMLElement | undefined = $state();
	let position = $state({ left: 0, top: 0, ready: false });

	const MARGIN = 8;
	// Swatches per row, for the up and down arrows
	const COLUMNS = 5;

	const current = $derived(calendar.color?.toLowerCase() ?? null);

	// Open at the point, moved inside the window if it would stick out; focus the current colour
	$effect(() => {
		if (!menu) return;
		const left = Math.min(x, window.innerWidth - menu.offsetWidth - MARGIN);
		const top = Math.min(y, window.innerHeight - menu.offsetHeight - MARGIN);
		position = { left: Math.max(MARGIN, left), top: Math.max(MARGIN, top), ready: true };
		const items = menuItems();
		(items.find((item) => item.getAttribute('aria-checked') === 'true') ?? items[0])?.focus();
	});

	function menuItems(): HTMLElement[] {
		return menu ? [...menu.querySelectorAll<HTMLElement>('[role^="menuitem"]')] : [];
	}

	function pick(color: string | null) {
		onpick(color);
		onclose(true);
	}

	// Arrows move through the swatches like a grid, then down the list below them
	function onmenukeydown(e: KeyboardEvent) {
		const items = menuItems();
		const index = items.indexOf(document.activeElement as HTMLElement);
		const inGrid = index < palette.length;
		const step: Record<string, number> = {
			ArrowRight: 1,
			ArrowLeft: -1,
			ArrowDown: inGrid ? COLUMNS : 1,
			ArrowUp: inGrid || index === palette.length ? -COLUMNS : -1
		};
		let next: number | undefined;
		if (e.key in step) {
			next = index + step[e.key];
			// Down from the last row of swatches lands on the first item below them
			if (inGrid && next > palette.length) next = palette.length;
			next = Math.min(items.length - 1, Math.max(0, next));
		} else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = items.length - 1;
		else if (e.key === 'Tab') onclose(true);
		if (next === undefined) return;
		e.preventDefault();
		items[next].focus();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') onclose(true);
	}

	function onpointerdown(e: PointerEvent) {
		if (!menu?.contains(e.target as Node)) onclose(false);
	}
</script>

<svelte:window
	{onkeydown}
	{onpointerdown}
	onresize={() => onclose(false)}
	onblur={() => onclose(false)}
/>

<div
	bind:this={menu}
	class={{ menu: true, ready: position.ready }}
	style:left="{position.left}px"
	style:top="{position.top}px"
	role="menu"
	tabindex="-1"
	aria-label={calendar.name}
	onkeydown={onmenukeydown}
>
	<div class="swatches">
		{#each palette as colour (colour.name)}
			<button
				type="button"
				role="menuitemradio"
				aria-checked={current === colour.hex}
				aria-label={colour.name}
				title={colour.name}
				tabindex="-1"
				class="swatch"
				style:--cal="var(--cal-{colour.name})"
				onclick={() => pick(colour.hex)}
			></button>
		{/each}
	</div>
	<button
		type="button"
		role="menuitemradio"
		aria-checked="false"
		tabindex="-1"
		class="default"
		onclick={() => pick(null)}
	>
		Default colour
	</button>
	{#if onrename && ondelete}
		<div class="separator" role="separator"></div>
		<button
			type="button"
			role="menuitem"
			tabindex="-1"
			class="default"
			onclick={() => {
				onclose(false);
				onrename();
			}}
		>
			Rename…
		</button>
		<button
			type="button"
			role="menuitem"
			tabindex="-1"
			class="default danger"
			onclick={() => {
				onclose(false);
				ondelete();
			}}
		>
			Delete calendar…
		</button>
	{/if}
</div>

<style>
	.menu {
		position: fixed;
		z-index: 10;
		display: flex;
		flex-direction: column;
		gap: var(--space-xs);
		padding: var(--space-sm);
		border-radius: var(--radius-lg);
		background: var(--surface-elevated);
		box-shadow: var(--shadow-2);
		visibility: hidden;
		transform-origin: left top;
	}

	.menu.ready {
		visibility: visible;
		animation: menu-in var(--duration-fast) var(--ease-standard) both;
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

	.swatches {
		display: grid;
		grid-template-columns: repeat(5, 28px);
		gap: var(--space-xs);
		padding: var(--space-xs);
	}

	.swatch {
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		padding: 0;
		border: 0;
		border-radius: var(--radius-full);
		background: transparent;
	}

	/* The colour is an inner dot, so the ring of the current one fits around it */
	.swatch::before {
		content: '';
		width: 20px;
		height: 20px;
		border-radius: var(--radius-full);
		background: var(--cal);
		transition: transform var(--duration-fast) var(--ease-standard);
	}

	.swatch:hover::before {
		transform: scale(1.1);
	}

	.swatch[aria-checked='true'] {
		box-shadow: inset 0 0 0 2px var(--cal);
	}

	.swatch:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 0;
	}

	.default {
		height: var(--control-height);
		padding: 0 var(--space-sm);
		border: 0;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--text);
		text-align: left;
		transition: background-color var(--duration-fast) var(--ease-standard);
	}

	.default:hover,
	.default:focus-visible {
		outline: none;
		background: var(--fill-hover);
	}

	.danger {
		color: var(--danger);
	}

	.separator {
		height: 1px;
		margin: var(--space-2xs) var(--space-sm);
		background: var(--separator);
	}
</style>
