//! Where things go, in logical points with the origin at the top left: the office window on the work
//! area, the tab and the drawer inside it, the desks, and a card beside a desk. The numbers are the
//! Swift app's `OfficeLayout` and `CardPlacement`, so the two draw the same office.

/// A rectangle in logical points, y down.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    /// A rectangle from its origin and size.
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// The right edge.
    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }

    /// The bottom edge.
    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }

    /// Whether the point is inside, edges included on the top and left only.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.max_x() && y >= self.y && y < self.max_y()
    }

    /// How far the point is from the rectangle, zero inside it.
    pub fn distance(&self, x: f64, y: f64) -> f64 {
        let dx = (self.x - x).max(0.0).max(x - self.max_x());
        let dy = (self.y - y).max(0.0).max(y - self.max_y());
        dx.hypot(dy)
    }
}

/// The drawer's width.
pub const WIDTH: f64 = 300.0;
/// The tab on the drawer's left edge, which stays on screen when it is shut.
pub const TAB_W: f64 = 26.0;
pub const TAB_H: f64 = 76.0;
pub const HEADER: f64 = 46.0;
pub const FOOTER: f64 = 52.0;
pub const INSET: f64 = 12.0;
pub const GAP: f64 = 10.0;
pub const DESK_W: f64 = (WIDTH - 2.0 * INSET - GAP) / 2.0;
pub const DESK_H: f64 = 158.0;
/// Room left of the tab inside the window for the CSS shadow.
pub const MARGIN: f64 = 12.0;
/// The office window's width: shadow margin, tab and drawer.
pub const WINDOW_W: f64 = MARGIN + TAB_W + WIDTH;
/// The office stays this much shorter than the work area.
pub const SCREEN_GAP: f64 = 24.0;
/// How far the card reaches over the drawer's rim.
pub const CARD_OVERLAP: f64 = 2.0;
/// How near the pointer has to be to the hit region for the fast poll.
pub const NEAR: f64 = 40.0;

/// The height of the desk area holding `desks` desks, two to a row.
pub fn desk_area_height(desks: usize) -> f64 {
    let rows = desks.div_ceil(2);
    rows as f64 * DESK_H + rows.saturating_sub(1) as f64 * GAP
}

/// The whole drawer's height for `desks` desks, or `available` when that is less.
pub fn height(desks: usize, available: f64) -> f64 {
    (HEADER + desk_area_height(desks) + INSET + FOOTER).min(available)
}

/// Desk `index`'s frame inside the desk area, which starts under the header.
pub fn desk_frame(index: usize) -> Rect {
    Rect::new(
        INSET + (index % 2) as f64 * (DESK_W + GAP),
        (index / 2) as f64 * (DESK_H + GAP),
        DESK_W,
        DESK_H,
    )
}

/// The office window's frame on `work` (the primary monitor's work area), for `desks` desks: against
/// the right edge, vertically centred.
pub fn office_frame(work: Rect, desks: usize) -> Rect {
    let h = height(desks, work.h - SCREEN_GAP).round();
    Rect::new(
        work.max_x() - WINDOW_W,
        work.y + ((work.h - h) / 2.0).round(),
        WINDOW_W,
        h,
    )
}

/// Where the drawer's left edge is inside the office window: beside the tab when open, past the
/// window's right edge when shut, so only the tab shows.
pub fn drawer_left(open: bool) -> f64 {
    if open { MARGIN + TAB_W } else { WINDOW_W }
}

/// The tab's rectangle inside an office window `h` tall.
pub fn tab_rect(open: bool, h: f64) -> Rect {
    Rect::new(drawer_left(open) - TAB_W, ((h - TAB_H) / 2.0).round(), TAB_W, TAB_H)
}

/// The parts of the office window that take the mouse: the tab, and the drawer while it is open.
pub fn hit_rects(open: bool, h: f64) -> Vec<Rect> {
    let mut rects = vec![tab_rect(open, h)];
    if open {
        rects.push(Rect::new(drawer_left(true), 0.0, WIDTH, h));
    }
    rects
}

/// The open drawer's frame on screen, given the office window's frame.
pub fn drawer_frame(office: Rect) -> Rect {
    Rect::new(office.max_x() - WIDTH, office.y, WIDTH, office.h)
}

/// The origin of a card `w` by `h` beside the open drawer, level with desk `seat` or, with no seat,
/// with the drawer's top, kept inside `work`.
pub fn card_origin(w: f64, h: f64, seat: Option<usize>, office: Rect, work: Rect) -> (f64, f64) {
    let drawer = drawer_frame(office);
    let x = (drawer.x - w + CARD_OVERLAP).max(work.x);
    let y = match seat {
        Some(seat) => {
            let desk = desk_frame(seat);
            drawer.y + HEADER + desk.y + desk.h / 2.0 - h / 2.0
        }
        None => drawer.y,
    };
    (x.round(), y.max(work.y).min(work.max_y() - h).round())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_desks_fit_one_row() {
        assert_eq!(DESK_W, 133.0);
        assert_eq!(desk_area_height(2), 158.0);
        assert_eq!(height(2, 1000.0), 46.0 + 158.0 + 12.0 + 52.0);
        assert_eq!(height(4, 300.0), 300.0);
    }

    #[test]
    fn office_sits_on_the_right_edge_centred() {
        let work = Rect::new(0.0, 25.0, 1440.0, 875.0);
        let office = office_frame(work, 2);
        assert_eq!(office, Rect::new(1440.0 - 338.0, 25.0 + 304.0, 338.0, 268.0));
    }

    #[test]
    fn tab_moves_with_the_drawer() {
        assert_eq!(tab_rect(false, 268.0), Rect::new(312.0, 96.0, 26.0, 76.0));
        assert_eq!(tab_rect(true, 268.0), Rect::new(12.0, 96.0, 26.0, 76.0));
        assert_eq!(hit_rects(true, 268.0)[1], Rect::new(38.0, 0.0, 300.0, 268.0));
    }

    #[test]
    fn card_stands_left_of_the_drawer() {
        let work = Rect::new(0.0, 25.0, 1440.0, 875.0);
        let office = office_frame(work, 2);
        let (x, y) = card_origin(480.0, 300.0, Some(1), office, work);
        assert_eq!(x, 1440.0 - 300.0 - 480.0 + 2.0);
        assert_eq!(y, office.y + 46.0 + 79.0 - 150.0);
        let (_, top) = card_origin(480.0, 300.0, None, office, work);
        assert_eq!(top, office.y);
        let (_, clamped) = card_origin(480.0, 600.0, None, office, work);
        assert_eq!(clamped, work.max_y() - 600.0);
    }

    #[test]
    fn distance_is_zero_inside() {
        let r = Rect::new(10.0, 10.0, 20.0, 20.0);
        assert_eq!(r.distance(15.0, 15.0), 0.0);
        assert_eq!(r.distance(0.0, 15.0), 10.0);
        assert_eq!(r.distance(33.0, 34.0), 5.0);
    }
}
