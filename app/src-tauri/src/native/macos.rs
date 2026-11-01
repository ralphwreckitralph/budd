//! Tauri's windows are `NSWindow`s of tao's own class. The office and the card need to be
//! `NSPanel`s with `NonactivatingPanel` so clicks and the keyboard reach them without activating
//! the app. The class of each window is swapped in place (`object_setClass`) for a panel subclass
//! that answers `canBecomeKeyWindow` the way its kind needs; the object's layout is unchanged since
//! the subclasses add no instance variables.

use std::ffi::c_void;
use std::sync::mpsc;

use objc2::class;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventType, NSPanel, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowLevel,
    NSWindowStyleMask,
};
use tauri::{Runtime, WebviewWindow};

use super::Kind;

/// `NSFloatingWindowLevel`.
const FLOATING: NSWindowLevel = 3;

define_class!(
    // SAFETY: NSPanel has no subclassing requirements beyond the main thread, and the struct has
    // no Drop and no instance variables, which `object_setClass` on an existing window relies on.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "BuddOfficePanel"]
    struct OfficePanel;

    impl OfficePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }
    }
);

define_class!(
    // SAFETY: as for OfficePanel.
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "BuddCardPanel"]
    struct CardPanel;

    impl CardPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            crate::card::interactive()
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            false
        }

        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if cfg!(debug_assertions) && event.r#type() == NSEventType::KeyDown {
                log::info!("card panel got keyDown keyCode={}", event.keyCode());
            }
            // SAFETY: forwarding the event to NSPanel's own implementation.
            unsafe { msg_send![super(self), sendEvent: event] }
        }
    }
);

unsafe extern "C" {
    fn object_setClass(obj: *mut AnyObject, cls: *const AnyClass) -> *const AnyClass;
}

/// The window's `NSWindow`, retained, from any thread that may touch it.
fn ns_window<R: Runtime>(window: &WebviewWindow<R>) -> Result<Retained<NSWindow>, String> {
    let raw: *mut c_void = window.ns_window().map_err(|e| e.to_string())?;
    // SAFETY: tauri hands out the live NSWindow pointer; retaining it keeps it alive for the call.
    unsafe { Retained::retain(raw.cast::<NSWindow>()) }.ok_or_else(|| "no NSWindow".into())
}

/// Makes the window a Budd panel of its kind. Must run on the main thread, before the first show.
pub fn convert<R: Runtime>(window: &WebviewWindow<R>, kind: Kind) -> Result<(), String> {
    let ns = ns_window(window)?;
    let class: &AnyClass = match kind {
        Kind::Office => OfficePanel::class(),
        Kind::Card => CardPanel::class(),
    };
    // SAFETY: the window is a live NSWindow on the main thread; the new class is an NSPanel
    // subclass with no added ivars, so the instance's memory layout still fits.
    unsafe {
        object_setClass(Retained::as_ptr(&ns).cast_mut().cast::<AnyObject>(), class);
    }
    let panel: &NSPanel = unsafe { &*Retained::as_ptr(&ns).cast::<NSPanel>() };
    panel.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
    // The class swap skipped NSPanel's init, and setStyleMask: alone does not tell the window
    // server that the window must not activate the app; the private setter does.
    let prevents = sel!(_setPreventsActivation:);
    if panel.respondsToSelector(prevents) {
        // SAFETY: the selector exists on this object and takes one BOOL.
        let _: () = unsafe { msg_send![panel, _setPreventsActivation: true] };
    }
    panel.setLevel(FLOATING);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    panel.setHidesOnDeactivate(false);
    panel.setMovable(false);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setHasShadow(false);
    Ok(())
}

/// Orders the window front on the main thread; with `key` it also takes the keyboard, which a
/// non-activating panel may do without activating the app. The card page is then told to focus
/// its compose box, after the window is key, so the first keystroke lands in it.
pub fn order_front<R: Runtime>(window: &WebviewWindow<R>, key: bool) -> Result<(), String> {
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let Ok(ns) = ns_window(&target) else {
                return;
            };
            if key {
                ns.makeKeyAndOrderFront(None);
                // Key status alone sends keystrokes to the window; the web view has to be the
                // first responder for them to reach the page.
                if let Some(web_view) = ns.contentView().and_then(|view| web_view_in(&view)) {
                    ns.makeFirstResponder(Some(&web_view));
                }
                let responder = ns.firstResponder().map(|r| {
                    let object: &NSObject = &r;
                    object.class().name().to_string_lossy().into_owned()
                });
                log::info!("card key={} first responder {responder:?}", ns.isKeyWindow());
                if let Err(error) = tauri::Emitter::emit_to(&target, target.label(), "focus_compose", ()) {
                    log::warn!("focus_compose not sent: {error}");
                }
            } else {
                ns.orderFrontRegardless();
            }
        })
        .map_err(|e| e.to_string())
}

/// Dev builds only: logs every key press NSApp dispatches, before any window sees it, to tell
/// keys that never reach the process from keys a window or view swallows.
pub fn install_key_monitor() {
    if !cfg!(debug_assertions) {
        return;
    }
    let block = block2::RcBlock::new(|event: std::ptr::NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit hands a live event for the duration of the call.
        let e = unsafe { event.as_ref() };
        log::debug!(
            "app got keyDown keyCode={} flags={:#x}",
            e.keyCode(),
            e.modifierFlags().bits()
        );
        event.as_ptr()
    });
    // SAFETY: the block returns the event it was given, which is what the API asks for; the
    // monitor lives as long as the app, so the returned token is dropped on purpose.
    let _ = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block) };
    std::mem::forget(block);
}

/// The first `WKWebView` under `view`, which is wry's, a level or two down.
fn web_view_in(view: &NSView) -> Option<Retained<NSView>> {
    let web_view_class = class!(WKWebView);
    for subview in view.subviews().iter() {
        if subview.isKindOfClass(web_view_class) {
            return Some(subview);
        }
        if let Some(found) = web_view_in(&subview) {
            return Some(found);
        }
    }
    None
}

/// The cursor in points from the primary display's top left. `mouseLocation` is a class method
/// any thread may call; it measures from the display's bottom left, so flip by `screen_h`.
pub fn cursor(screen_h: f64) -> Option<(f64, f64)> {
    let point = NSEvent::mouseLocation();
    Some((point.x, screen_h - point.y))
}

/// The panel's class, style mask, level, collection behaviour and key state, read on the main thread.
pub fn describe<R: Runtime>(window: &WebviewWindow<R>) -> Result<String, String> {
    let target = window.clone();
    let (tx, rx) = mpsc::channel();
    window
        .run_on_main_thread(move || {
            let text = ns_window(&target).map(|ns| {
                let object: &NSObject = &ns;
                format!(
                    "class={} styleMask={:#x} level={} collection={:#x} canBecomeKey={} isKey={} visible={} ignoresMouse={}",
                    object.class().name().to_string_lossy(),
                    ns.styleMask().bits(),
                    ns.level(),
                    ns.collectionBehavior().bits(),
                    ns.canBecomeKeyWindow(),
                    ns.isKeyWindow(),
                    ns.isVisible(),
                    ns.ignoresMouseEvents(),
                )
            });
            let _ = tx.send(text);
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}
