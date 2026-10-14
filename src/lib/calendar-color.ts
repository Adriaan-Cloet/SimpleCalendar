// Turns a calendar's colour into a CSS value for `--cal` (design.md, "Palette"). A palette name,
// or a hex value equal to a palette colour's light value, maps to its token, so it switches
// between the light and dark value. Any other hex, such as a colour Google sends, is used as is.

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

export function calendarColor(color: string): string {
	if (!color.startsWith('#')) return `var(--cal-${color})`;
	const name = paletteByLightHex[color.toLowerCase()];
	return name ? `var(--cal-${name})` : color;
}
