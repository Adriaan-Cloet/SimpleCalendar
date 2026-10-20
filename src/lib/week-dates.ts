// Date helpers for the views. They work on local calendar dates, to find the days to show and
// name them; they do nothing with zones or recurrence, which stays in Rust (architecture §10).

export function startOfWeek(date: Date): Date {
	const monday = new Date(date.getFullYear(), date.getMonth(), date.getDate());
	// getDay() is 0 for Sunday; shift so Monday is the first day
	monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
	return monday;
}

export function addDays(date: Date, days: number): Date {
	const result = new Date(date);
	result.setDate(result.getDate() + days);
	return result;
}

export function isSameDay(a: Date, b: Date): boolean {
	return (
		a.getFullYear() === b.getFullYear() &&
		a.getMonth() === b.getMonth() &&
		a.getDate() === b.getDate()
	);
}

export function minutesSinceMidnight(date: Date): number {
	return date.getHours() * 60 + date.getMinutes();
}

export function atMinutes(day: Date, minutes: number): Date {
	return new Date(day.getFullYear(), day.getMonth(), day.getDate(), 0, minutes);
}

/** The local date as `YYYY-MM-DD`, the form `list_occurrences` takes */
export function isoDate(date: Date): string {
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}
