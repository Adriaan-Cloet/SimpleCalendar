// Scrolling an event into view, for a reminder that opens it in the day view (#25). Pure, so
// the grid only measures and the arithmetic is tested.

export interface RevealBox {
	/** The scroller's current `scrollTop` */
	scrollTop: number;
	/** The height of the scroller's visible area */
	viewHeight: number;
	/** The height of the sticky header that covers the top of that area */
	covered: number;
	/** The event's top and bottom, measured from the top of the scroller's content */
	top: number;
	bottom: number;
	/** Room to leave above the event, so what comes before it shows too */
	margin: number;
}

/**
 * The `scrollTop` that shows the whole event below the sticky header, with `margin` above it,
 * or `null` if it already shows. An event taller than the view shows from its start.
 */
export function revealScrollTop(box: RevealBox): number | null {
	const visibleTop = box.scrollTop + box.covered;
	const visibleBottom = box.scrollTop + box.viewHeight;
	if (box.top >= visibleTop && box.bottom <= visibleBottom) return null;
	return Math.max(0, box.top - box.covered - box.margin);
}
