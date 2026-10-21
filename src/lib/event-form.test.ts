import { describe, expect, it } from 'vitest';
import type { EventDetail } from './calendar.js';
import {
	defaultStart,
	draftFromForm,
	formFromEvent,
	formProblem,
	newEventForm,
	reminderChoices,
	reminderLabel,
	shiftDate,
	withAllDay,
	withStart,
	zoneOptions,
	type EventForm
} from './event-form.js';

const form = (changes: Partial<EventForm> = {}): EventForm => ({
	calendarId: 1,
	title: 'Meeting',
	location: '',
	notes: '',
	allDay: false,
	startDate: '2026-10-20',
	startTime: '09:00',
	endDate: '2026-10-20',
	endTime: '10:30',
	timeZone: 'Europe/Brussels',
	useDefaultReminders: false,
	reminders: [],
	...changes
});

const detail = (changes: Partial<EventDetail> = {}): EventDetail => ({
	id: 7,
	calendar_id: 2,
	title: 'Lunch',
	description: 'Bring bread',
	location: 'Ghent',
	all_day: false,
	start: '2026-10-21T12:00',
	end: '2026-10-21T13:15',
	time_zone: 'Asia/Tokyo',
	use_default_reminders: false,
	reminders: [{ minutes_before: 10, method: 'popup' }],
	editable: true,
	recurring: false,
	...changes
});

describe('newEventForm', () => {
	it('starts a timed event an hour long', () => {
		const f = newEventForm({
			calendarId: 3,
			start: new Date(2026, 9, 20, 23, 30),
			allDay: false,
			timeZone: 'Europe/Brussels'
		});
		expect(f).toMatchObject({
			calendarId: 3,
			title: '',
			allDay: false,
			startDate: '2026-10-20',
			startTime: '23:30',
			endDate: '2026-10-21',
			endTime: '00:30',
			timeZone: 'Europe/Brussels',
			useDefaultReminders: false,
			reminders: []
		});
	});

	it('starts an all-day event on one day, with times ready for when it stops being all-day', () => {
		const f = newEventForm({
			calendarId: 3,
			start: new Date(2026, 11, 25),
			allDay: true,
			timeZone: 'UTC'
		});
		expect(f).toMatchObject({
			allDay: true,
			startDate: '2026-12-25',
			endDate: '2026-12-25',
			startTime: '09:00',
			endTime: '10:00'
		});
	});
});

describe('formFromEvent and draftFromForm', () => {
	it('round-trips a timed event', () => {
		const f = formFromEvent(detail(), 'Europe/Brussels');
		expect(f).toEqual({
			calendarId: 2,
			title: 'Lunch',
			location: 'Ghent',
			notes: 'Bring bread',
			allDay: false,
			startDate: '2026-10-21',
			startTime: '12:00',
			endDate: '2026-10-21',
			endTime: '13:15',
			timeZone: 'Asia/Tokyo',
			useDefaultReminders: false,
			reminders: [{ minutes_before: 10, method: 'popup' }]
		});
		expect(draftFromForm(f)).toEqual({
			calendar_id: 2,
			title: 'Lunch',
			description: 'Bring bread',
			location: 'Ghent',
			all_day: false,
			start: '2026-10-21T12:00',
			end: '2026-10-21T13:15',
			time_zone: 'Asia/Tokyo',
			use_default_reminders: false,
			reminders: [{ minutes_before: 10, method: 'popup' }]
		});
	});

	it('shows the last day of an all-day event and sends the day after', () => {
		const f = formFromEvent(
			detail({ all_day: true, start: '2026-10-20', end: '2026-10-23', time_zone: null }),
			'Europe/Brussels'
		);
		expect(f).toMatchObject({
			allDay: true,
			startDate: '2026-10-20',
			endDate: '2026-10-22',
			startTime: '09:00',
			endTime: '10:00',
			timeZone: 'Europe/Brussels'
		});
		expect(draftFromForm(f)).toMatchObject({
			all_day: true,
			start: '2026-10-20',
			end: '2026-10-23',
			time_zone: null
		});
	});

	it('uses the display zone for an event without one', () => {
		const f = formFromEvent(detail({ time_zone: null }), 'Europe/Brussels');
		expect(f.timeZone).toBe('Europe/Brussels');
	});

	it('sends empty text as null', () => {
		const d = draftFromForm(form({ location: '  ', notes: '' }));
		expect(d.location).toBeNull();
		expect(d.description).toBeNull();
	});
});

describe('withStart', () => {
	it('keeps the length of a timed event', () => {
		const f = withStart(form(), '2026-10-31', '23:00');
		expect(f).toMatchObject({
			startDate: '2026-10-31',
			startTime: '23:00',
			endDate: '2026-11-01',
			endTime: '00:30'
		});
	});

	it('counts the length on the wall clock, also across a DST change', () => {
		// 25 October 2026 is 25 hours long in Brussels; the event still keeps 1h30 on the clock
		const f = withStart(
			form({ startDate: '2026-10-24', endDate: '2026-10-24' }),
			'2026-10-25',
			'01:00'
		);
		expect(f).toMatchObject({ endDate: '2026-10-25', endTime: '02:30' });
	});

	it('keeps the number of days of an all-day event', () => {
		const f = withStart(
			form({ allDay: true, startDate: '2026-10-20', endDate: '2026-10-22' }),
			'2026-10-30',
			'09:00'
		);
		expect(f).toMatchObject({ startDate: '2026-10-30', endDate: '2026-11-01' });
	});

	it('leaves the end alone while the start cannot be read', () => {
		const f = withStart(form(), '', '09:00');
		expect(f).toMatchObject({ startDate: '', endDate: '2026-10-20', endTime: '10:30' });
	});
});

describe('withAllDay', () => {
	it('switches back and forth without losing the times', () => {
		const allDay = withAllDay(form(), true);
		expect(allDay).toMatchObject({ allDay: true, startTime: '09:00', endTime: '10:30' });
		expect(withAllDay(allDay, false)).toEqual(form());
	});
});

describe('formProblem', () => {
	it('accepts a normal event and a zero-length one', () => {
		expect(formProblem(form())).toBeNull();
		expect(formProblem(form({ endTime: '09:00' }))).toBeNull();
	});

	it('refuses an end before the start', () => {
		expect(formProblem(form({ endTime: '08:59' }))).toMatch(/before/);
		expect(formProblem(form({ endDate: '2026-10-19', endTime: '23:00' }))).toMatch(/before/);
		expect(
			formProblem(form({ allDay: true, startDate: '2026-10-20', endDate: '2026-10-19' }))
		).toMatch(/before/);
	});

	it('ignores the times of an all-day event', () => {
		expect(formProblem(form({ allDay: true, endTime: '08:00' }))).toBeNull();
	});

	it('asks for a date and time it can read', () => {
		expect(formProblem(form({ startDate: '' }))).toMatch(/date/);
		expect(formProblem(form({ startDate: '20/10/2026' }))).toMatch(/date/);
		expect(formProblem(form({ endTime: '9:00' }))).toMatch(/time/);
		expect(formProblem(form({ endTime: '24:00' }))).toMatch(/time/);
		expect(formProblem(form({ startDate: '2026-02-30' }))).toMatch(/date/);
	});
});

describe('reminders', () => {
	it('names every choice', () => {
		expect(reminderChoices.map((m) => reminderLabel(m))).toEqual([
			'At time of event',
			'5 minutes before',
			'10 minutes before',
			'15 minutes before',
			'30 minutes before',
			'1 hour before',
			'2 hours before',
			'1 day before',
			'2 days before',
			'1 week before'
		]);
	});

	it('names other values in the largest whole unit', () => {
		expect(reminderLabel(1)).toBe('1 minute before');
		expect(reminderLabel(90)).toBe('90 minutes before');
		expect(reminderLabel(180)).toBe('3 hours before');
		expect(reminderLabel(4320)).toBe('3 days before');
		expect(reminderLabel(40320)).toBe('4 weeks before');
	});
});

describe('shiftDate', () => {
	it('moves across months, years and DST changes', () => {
		expect(shiftDate('2026-10-31', 1)).toBe('2026-11-01');
		expect(shiftDate('2026-01-01', -1)).toBe('2025-12-31');
		expect(shiftDate('2026-03-28', 2)).toBe('2026-03-30');
	});
});

describe('defaultStart', () => {
	it('is the next whole hour in the current week', () => {
		const now = new Date(2026, 9, 21, 14, 20);
		expect(defaultStart(new Date(2026, 9, 19), now)).toEqual(new Date(2026, 9, 21, 15, 0));
	});

	it('is 09:00 on the first day of another week', () => {
		const now = new Date(2026, 9, 21, 14, 20);
		expect(defaultStart(new Date(2026, 9, 26), now)).toEqual(new Date(2026, 9, 26, 9, 0));
	});
});

describe('zoneOptions', () => {
	it('adds the event and display zones to the supported list, sorted, each once', () => {
		expect(zoneOptions(['Europe/Paris', 'Asia/Tokyo'], 'Etc/GMT+5', 'Asia/Tokyo')).toEqual([
			'Asia/Tokyo',
			'Etc/GMT+5',
			'Europe/Paris',
			'UTC'
		]);
	});

	it('always offers UTC', () => {
		expect(zoneOptions([], 'Europe/Paris', 'Europe/Paris')).toEqual(['Europe/Paris', 'UTC']);
	});
});
