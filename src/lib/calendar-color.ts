// Turns a calendar's colour into a CSS value for `--cal` (design.md, "Palette"). A palette name,
// or a hex value equal to a palette colour's light value, maps to its token, so it switches
// between the light and dark value. Any other hex, such as a colour Google sends, is used as is.
// A calendar without a colour, as most ICS feeds are, gets a palette colour picked by its id, so
// it keeps the same one and two such calendars rarely share one.

/** The colours the user can pick, in menu order. `hex` is the light value, which is what gets
 * stored as a calendar's colour. */
export const palette = [
	{ name: 'red', hex: '#ff3b30' },
	{ name: 'orange', hex: '#ff9500' },
	{ name: 'yellow', hex: '#ffcc00' },
	{ name: 'green', hex: '#34c759' },
	{ name: 'teal', hex: '#30b0c7' },
	{ name: 'blue', hex: '#007aff' },
	{ name: 'indigo', hex: '#5856d6' },
	{ name: 'purple', hex: '#af52de' },
	{ name: 'pink', hex: '#ff2d55' },
	{ name: 'brown', hex: '#a2845e' }
] as const;

const paletteByLightHex: Record<string, string> = Object.fromEntries(
	palette.map(({ name, hex }) => [hex, name])
);

// Blue first: the colour of calendar 1 is the accent
const fallbackOrder = [
	'blue',
	'orange',
	'green',
	'purple',
	'teal',
	'red',
	'indigo',
	'pink',
	'yellow',
	'brown'
];

export function calendarColor(color: string | null, calendarId: number): string {
	if (!color) return `var(--cal-${fallbackOrder[(calendarId - 1) % fallbackOrder.length]})`;
	if (!color.startsWith('#')) return `var(--cal-${color})`;
	const name = paletteByLightHex[color.toLowerCase()];
	return name ? `var(--cal-${name})` : color;
}
