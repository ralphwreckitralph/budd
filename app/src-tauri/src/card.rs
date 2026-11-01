//! The card window: shown beside a desk, with the keyboard only when a human asked for it.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Deserialize;
use tauri::{AppHandle, LogicalPosition, Manager, Runtime};

use crate::layout;
use crate::native;
use crate::shell::{self, CARD, Shell};

/// The card's size until it reports its own.
pub const WIDTH: f64 = 480.0;
pub const HEIGHT: f64 = 300.0;

/// Whether the card may become the key window right now.
static INTERACTIVE: AtomicBool = AtomicBool::new(false);

/// Whether the card is in its interactive state, read by the macOS panel class.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn interactive() -> bool {
    INTERACTIVE.load(Ordering::Relaxed)
}

/// What `card_show` takes: the desk to stand beside (none for the office's top), whether the card
/// takes the mouse and the keyboard at all, and whether to give it the keyboard now.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Show {
    pub seat: Option<usize>,
    pub interactive: bool,
    pub focus: bool,
}

/// Places the card beside the desk and shows it. The office opens first if it was shut, since the
/// card stands beside the open drawer. With `focus`, the card becomes key without activating the app
/// and the page is told to put the caret in the compose box.
pub fn show<R: Runtime>(app: &AppHandle<R>, show: Show) -> Result<(), String> {
    let shell = app.state::<Shell>();
    if !shell.is_open() {
        shell::set_open(app, true);
    }
    let placement = shell.placement();
    let (x, y) = layout::card_origin(WIDTH, HEIGHT, show.seat, placement.office, placement.work);
    let window = app.get_webview_window(CARD).ok_or("no card window")?;
    window
        .set_position(LogicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    INTERACTIVE.store(show.interactive, Ordering::Relaxed);
    window
        .set_ignore_cursor_events(!show.interactive)
        .map_err(|e| e.to_string())?;
    native::order_front(&window, show.interactive && show.focus)
}

/// Hides the card. The keyboard goes back to whatever had it, since the app never activated.
pub fn hide<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let window = app.get_webview_window(CARD).ok_or("no card window")?;
    INTERACTIVE.store(false, Ordering::Relaxed);
    window.hide().map_err(|e| e.to_string())
}
