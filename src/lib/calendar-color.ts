// Turns a calendar's colour into a CSS value for `--cal` (design.md, "Palette"). A palette name,
// or a hex value equal to a palette colour's light value, maps to its token, so it switches
// between the light and dark value. Any other hex, such as a colour Google sends, is used as is.
// A calendar without a colour, as most ICS feeds are, gets a palette colour picked by its id, so
// it keeps the same one and two such calendars rarely share one.

const paletteByLightHex: Record<string, string> = {
	'#ff3b30': 'red',
	'#ff9500': 'orange',
	'#ffcc00': 'yellow',
	'#34c759': 'green',
	'#30b0c7': 'teal',
	'#007aff': 'blue',
	'#5856d6': 'indigo',
	'#af52de': 'purple',
	'#ff2d55': 'pink',
	'#a2845e': 'brown'
};

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
