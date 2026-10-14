// Fake data for the design prototype. Delete this file once `list_occurrences` exists.
//
// Days are offsets from the Monday of the current week (0 = Monday), so "today" always falls
// inside the data. Times are minutes since midnight.

export type PaletteColor =
	'red' | 'orange' | 'yellow' | 'green' | 'teal' | 'blue' | 'indigo' | 'purple' | 'pink' | 'brown';

export type SourceKind = 'local' | 'google' | 'ics';

export interface Calendar {
	id: string;
	name: string;
	source: SourceKind;
	/** A palette name, or a raw hex value as a provider such as Google sends it. */
	color: PaletteColor | `#${string}`;
	editable: boolean;
}

export interface TimedEvent {
	id: string;
	calendarId: string;
	title: string;
	day: number;
	start: number;
	end: number;
	// Hand-placed here. The real values come from the tested overlap layout function
	// (architecture.md §10).
	column: number;
	columns: number;
	location?: string;
	notes?: string;
}

export interface AllDayEvent {
	id: string;
	calendarId: string;
	title: string;
	firstDay: number;
	lastDay: number;
	/** Row inside the all-day area, hand-placed like `column` above. */
	row: number;
}

export const sourceLabels: Record<SourceKind, string> = {
	local: 'On this computer',
	google: 'Google',
	ics: 'Subscriptions'
};

export const calendars: Calendar[] = [
	{ id: 'personal', name: 'Personal', source: 'local', color: 'orange', editable: true },
	{ id: 'work', name: 'Work', source: 'google', color: 'blue', editable: true },
	{ id: 'training', name: 'Training', source: 'google', color: 'green', editable: true },
	// Google's "Flamingo": a colour outside our palette, to show the fallback works
	{ id: 'family', name: 'Family', source: 'google', color: '#e67c73', editable: false },
	{ id: 'club', name: 'Club fixtures', source: 'ics', color: 'indigo', editable: false }
];

function t(time: string): number {
	const [h, m] = time.split(':').map(Number);
	return h * 60 + m;
}

function timed(
	id: string,
	calendarId: string,
	title: string,
	day: number,
	start: string,
	end: string,
	extra: Partial<TimedEvent> = {}
): TimedEvent {
	return {
		id,
		calendarId,
		title,
		day,
		start: t(start),
		end: t(end),
		column: 0,
		columns: 1,
		...extra
	};
}

export const timedEvents: TimedEvent[] = [
	// Monday
	timed('m1', 'work', 'Standup', 0, '09:00', '09:15'),
	timed('m2', 'work', 'Sprint planning', 0, '10:00', '11:30', { location: 'Room 2.14' }),
	timed('m3', 'personal', 'Lunch with Sam', 0, '12:30', '13:15', { location: 'De Markt' }),
	timed('m4', 'training', 'Run 10 km', 0, '18:30', '19:30', { notes: 'Easy pace, zone 2.' }),

	// Tuesday: two events overlap
	timed('t1', 'work', 'Standup', 1, '09:00', '09:15'),
	timed('t2', 'work', 'Design review', 1, '11:00', '12:00', { column: 0, columns: 2 }),
	timed('t3', 'work', '1:1 with Lena', 1, '11:30', '12:30', { column: 1, columns: 2 }),
	timed('t4', 'work', 'Focus time', 1, '14:00', '16:00'),
	timed('t5', 'training', 'Swim 2 km', 1, '19:00', '20:30', { location: 'Sportoase' }),

	// Wednesday: three events overlap, one long block
	timed('w1', 'work', 'Standup', 2, '09:00', '09:15'),
	timed('w2', 'work', 'Customer call', 2, '10:00', '11:00', { column: 0, columns: 3 }),
	timed('w3', 'personal', 'Dentist', 2, '10:00', '10:45', {
		column: 1,
		columns: 3,
		location: 'Bondgenotenlaan 12'
	}),
	timed('w4', 'work', 'Interview', 2, '10:30', '11:30', { column: 2, columns: 3 }),
	timed('w5', 'work', 'Workshop: offline sync', 2, '13:00', '17:00', {
		location: 'Room 3.02',
		notes: 'Bring laptop. We go through the outbox design and the conflict rules.'
	}),
	timed('w6', 'club', 'League match, home', 2, '20:00', '21:45', { location: 'Stadium' }),

	// Thursday: starts before the initial scroll position
	timed('h1', 'training', 'Bike intervals', 3, '06:15', '07:15'),
	timed('h2', 'work', 'Standup', 3, '09:00', '09:15'),
	timed('h3', 'work', 'Team lunch', 3, '12:00', '13:00'),
	timed('h4', 'personal', 'Haircut', 3, '15:30', '16:00'),

	// Friday: ends late
	timed('f1', 'work', 'Standup', 4, '09:00', '09:15'),
	timed('f2', 'work', 'Release review', 4, '10:00', '12:00'),
	timed('f3', 'work', 'Drinks', 4, '16:30', '18:00'),
	timed('f4', 'personal', 'Cinema', 4, '21:30', '23:30'),

	// Weekend
	timed('s1', 'training', 'Long ride 90 km', 5, '08:00', '11:00'),
	timed('s2', 'family', 'Birthday lunch, Grandma', 5, '12:30', '15:00', { location: 'Home' }),
	timed('s3', 'club', 'League match, away', 5, '18:30', '20:15'),
	timed('u1', 'training', 'Long run', 6, '09:00', '10:30'),
	timed('u2', 'family', 'Family brunch', 6, '11:30', '13:30')
];

export const allDayEvents: AllDayEvent[] = [
	{ id: 'a1', calendarId: 'work', title: 'Conference, Ghent', firstDay: 1, lastDay: 3, row: 0 },
	{ id: 'a2', calendarId: 'family', title: 'Mum’s birthday', firstDay: 0, lastDay: 0, row: 0 },
	{ id: 'a3', calendarId: 'personal', title: 'Pay rent', firstDay: 4, lastDay: 4, row: 0 }
];
