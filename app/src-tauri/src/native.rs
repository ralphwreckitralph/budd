//! The platform glue behind the two windows: on macOS they become non-activating panels, on Windows
//! tool windows that never take activation. Other platforms get Tauri's plain behaviour.

use tauri::{Runtime, WebviewWindow};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod win;

/// Which window is being set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Never takes the keyboard.
    Office,
    /// Takes the keyboard only while a card is interactive.
    Card,
}

/// Turns a freshly built, still hidden window into the floating panel its kind needs. Main thread.
pub fn convert<R: Runtime>(window: &WebviewWindow<R>, kind: Kind) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::convert(window, kind);
    #[cfg(windows)]
    return win::convert(window, kind);
    #[allow(unreachable_code)]
    {
        let _ = (window, kind);
        Ok(())
    }
}

/// Shows the window in front of everything. With `key` it takes the keyboard too, without activating
/// the app; without it, nothing about the keyboard changes.
pub fn order_front<R: Runtime>(window: &WebviewWindow<R>, key: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::order_front(window, key);
    #[allow(unreachable_code)]
    {
        window.show().map_err(|e| e.to_string())?;
        if key {
            window.set_focus().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

/// The cursor in logical points from the primary display's top left, read without the main
/// thread; `None` where only Tauri's getter can tell. `screen_h` is the primary display's height
/// in points and `scale` its pixels per point.
pub fn cursor(screen_h: f64, scale: f64) -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        let _ = scale;
        macos::cursor(screen_h)
    }
    #[cfg(windows)]
    {
        let _ = screen_h;
        win::cursor(scale)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (screen_h, scale);
        None
    }
}

/// Dev builds only: logs the key presses the process receives, for the focus exit test.
pub fn install_key_monitor() {
    #[cfg(target_os = "macos")]
    macos::install_key_monitor();
}

/// The window's native state in one line, for the debug channel and the exit tests.
pub fn describe<R: Runtime>(window: &WebviewWindow<R>) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    return macos::describe(window);
    #[cfg(windows)]
    return win::describe(window);
    #[allow(unreachable_code)]
    Ok(format!("visible={}", window.is_visible().map_err(|e| e.to_string())?))
}
