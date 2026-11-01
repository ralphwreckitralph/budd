//! On Windows both windows are tool windows (no taskbar button, no Alt-Tab), and the office never
//! activates: `WS_EX_NOACTIVATE` plus a subclass answering `WM_MOUSEACTIVATE` with `MA_NOACTIVATE`,
//! so a click on the tab leaves the keyboard where it was. Compiled, not yet verified on Windows.

#![allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]

use tauri::{Runtime, WebviewWindow};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetCursorPos, GetWindowLongPtrW, MA_NOACTIVATE, SetWindowLongPtrW, WM_MOUSEACTIVATE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

use super::Kind;

/// The subclass id, one per window, unique within the app.
const SUBCLASS_ID: usize = 0x0b0d;

unsafe extern "system" fn no_activate(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    // SAFETY: forwarding to the next procedure in the chain, as every subclass must.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Sets the extended styles and, for the office, installs the no-activate subclass.
pub fn convert<R: Runtime>(window: &WebviewWindow<R>, kind: Kind) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    // SAFETY: hwnd is the live handle of a window this process owns.
    unsafe {
        let mut style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) | WS_EX_TOOLWINDOW.0 as isize;
        if kind == Kind::Office {
            style |= WS_EX_NOACTIVATE.0 as isize;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
        if kind == Kind::Office && !SetWindowSubclass(hwnd, Some(no_activate), SUBCLASS_ID, 0).as_bool() {
            return Err("SetWindowSubclass failed".into());
        }
    }
    Ok(())
}

/// The cursor in logical points, from its physical position and the primary monitor's scale.
pub fn cursor(scale: f64) -> Option<(f64, f64)> {
    let mut point = POINT::default();
    // SAFETY: a valid out-pointer to a POINT.
    unsafe { GetCursorPos(&raw mut point) }.ok()?;
    Some((f64::from(point.x) / scale, f64::from(point.y) / scale))
}

/// The window's extended style bits and visibility.
pub fn describe<R: Runtime>(window: &WebviewWindow<R>) -> Result<String, String> {
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    // SAFETY: as in convert.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    Ok(format!(
        "exStyle={:#x} visible={}",
        style,
        window.is_visible().map_err(|e| e.to_string())?
    ))
}
