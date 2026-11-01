//! The office window's state: open or shut, how many desks, where it is on screen, and the loop that
//! lets clicks through everywhere but the tab and the open drawer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime, WebviewWindow};

use crate::layout::{self, Rect};

/// The office window's label.
pub const OFFICE: &str = "office";
/// The card window's label.
pub const CARD: &str = "card";

/// What the office page needs to know to draw the right thing.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct OfficeState {
    pub open: bool,
    pub desks: usize,
}

/// Where the office window stands, in logical points.
#[derive(Clone, Copy, Debug, Default)]
pub struct Placement {
    /// The primary monitor's work area.
    pub work: Rect,
    /// The office window's frame.
    pub office: Rect,
    /// Physical pixels per logical point on that monitor.
    pub scale: f64,
    /// The primary display's full height in logical points, which macOS measures the cursor from.
    pub screen_h: f64,
}

/// The shell's shared state, managed by Tauri.
#[derive(Default)]
pub struct Shell {
    open: AtomicBool,
    desks: AtomicUsize,
    placement: Mutex<Placement>,
}

impl Shell {
    /// Two desks until the core says otherwise.
    pub fn new() -> Self {
        Self {
            open: AtomicBool::new(false),
            desks: AtomicUsize::new(2),
            placement: Mutex::new(Placement::default()),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    pub fn desks(&self) -> usize {
        self.desks.load(Ordering::Relaxed)
    }

    pub fn state(&self) -> OfficeState {
        OfficeState {
            open: self.is_open(),
            desks: self.desks(),
        }
    }

    pub fn placement(&self) -> Placement {
        *self.placement.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn set_placement(&self, placement: Placement) {
        *self.placement.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = placement;
    }
}

/// Opens or shuts the drawer and tells the office page.
pub fn set_open<R: Runtime>(app: &AppHandle<R>, open: bool) -> OfficeState {
    let shell = app.state::<Shell>();
    shell.open.store(open, Ordering::Relaxed);
    let state = shell.state();
    log::info!("office {}", if open { "open" } else { "shut" });
    if let Err(error) = app.emit_to(OFFICE, "office", state) {
        log::warn!("office state not sent: {error}");
    }
    state
}

/// The primary monitor's work area and scale, in logical points.
pub fn work_area<R: Runtime>(app: &AppHandle<R>) -> Option<(Rect, f64)> {
    screen(app).map(|(work, scale, _)| (work, scale))
}

/// The primary monitor's work area, scale, and full height, in logical points.
fn screen<R: Runtime>(app: &AppHandle<R>) -> Option<(Rect, f64, f64)> {
    let monitor = app.primary_monitor().ok().flatten()?;
    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    Some((
        Rect::new(
            f64::from(area.position.x) / scale,
            f64::from(area.position.y) / scale,
            f64::from(area.size.width) / scale,
            f64::from(area.size.height) / scale,
        ),
        scale,
        f64::from(monitor.size().height) / scale,
    ))
}

/// Puts the office window where it belongs on the primary monitor, and remembers where that is.
/// Moving it only when the frame changed keeps the 2 s check free of window traffic.
pub fn place<R: Runtime>(app: &AppHandle<R>) {
    let Some((work, scale, screen_h)) = screen(app) else {
        return;
    };
    let shell = app.state::<Shell>();
    let office = layout::office_frame(work, shell.desks());
    let before = shell.placement();
    shell.set_placement(Placement {
        work,
        office,
        scale,
        screen_h,
    });
    if before.office == office && before.scale == scale {
        return;
    }
    log::info!("office at {office:?} on work area {work:?} at {scale}x");
    let Some(window) = app.get_webview_window(OFFICE) else {
        return;
    };
    if before.office.w != office.w || before.office.h != office.h {
        let _ = window.set_size(LogicalSize::new(office.w, office.h));
    }
    let _ = window.set_position(LogicalPosition::new(office.x, office.y));
}

/// The pointer, in the drawer's coordinates: the tab spans x -26 to 0.
#[derive(Clone, Copy, Debug, Serialize)]
struct Pointer {
    x: f64,
    y: f64,
}

/// Polls the cursor at 30 Hz, 60 Hz near the hit region, and lets clicks through the office window
/// everywhere except the tab and the open drawer. While the cursor is over the window, the office
/// page gets a `pointer` event each poll, and one `null` when it leaves. Runs for the app's life.
pub fn run_pointer_loop<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut ignoring: Option<bool> = None;
        let mut inside = false;
        let mut placed = Instant::now();
        place(&app);
        loop {
            if placed.elapsed() >= Duration::from_secs(2) {
                place(&app);
                placed = Instant::now();
            }
            let Some(window) = app.get_webview_window(OFFICE) else {
                return;
            };
            let shell = app.state::<Shell>();
            let placement = shell.placement();
            let office = placement.office;
            let open = shell.is_open();
            let cursor = cursor(&app, &placement).map(|(x, y)| (x - office.x, y - office.y));
            let mut near = false;
            let mut hit = false;
            let mut over = false;
            if let Some((x, y)) = cursor {
                over = x >= 0.0 && x < office.w && y >= 0.0 && y < office.h;
                for rect in layout::hit_rects(open, office.h) {
                    let distance = rect.distance(x, y);
                    hit |= distance == 0.0;
                    near |= distance < layout::NEAR;
                }
            }
            apply_ignore(&window, &mut ignoring, !hit);
            if over {
                inside = true;
                let (x, y) = cursor.unwrap_or_default();
                let pointer = Pointer {
                    x: x - layout::drawer_left(open),
                    y,
                };
                let _ = app.emit_to(OFFICE, "pointer", Some(pointer));
            } else if inside {
                inside = false;
                let _ = app.emit_to(OFFICE, "pointer", None::<Pointer>);
            }
            tokio::time::sleep(Duration::from_millis(if near { 16 } else { 33 })).await;
        }
    });
}

/// The cursor in logical points from the primary display's top left: read natively where the
/// platform lets another thread do that, since Tauri's own getter is a round trip to the main
/// thread, too dear at 30 Hz.
fn cursor<R: Runtime>(app: &AppHandle<R>, placement: &Placement) -> Option<(f64, f64)> {
    crate::native::cursor(placement.screen_h, placement.scale).or_else(|| {
        let c = app.cursor_position().ok()?;
        Some((c.x / placement.scale, c.y / placement.scale))
    })
}

/// Sets whether the office ignores the mouse, only when that changes.
fn apply_ignore<R: Runtime>(window: &WebviewWindow<R>, current: &mut Option<bool>, ignore: bool) {
    if *current == Some(ignore) {
        return;
    }
    match window.set_ignore_cursor_events(ignore) {
        Ok(()) => *current = Some(ignore),
        Err(error) => log::warn!("click-through not set: {error}"),
    }
}
