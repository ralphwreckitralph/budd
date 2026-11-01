//! Budd's office shell: a floating, non-activating drawer on the right edge of the main screen and a
//! card window that takes the keyboard only when a human opens it. This is the window model alone;
//! the desks and the cards' content come with later milestones.

pub mod layout;

mod card;
mod debug;
mod native;
mod shell;

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_log::{Target, TargetKind};

use shell::{CARD, OFFICE, OfficeState, Shell};

/// Toggles the drawer, from a click on the tab.
#[tauri::command]
fn office_toggle<R: Runtime>(app: AppHandle<R>) -> OfficeState {
    let open = !app.state::<Shell>().is_open();
    shell::set_open(&app, open)
}

/// The drawer's state, for a page that just loaded.
#[tauri::command]
fn office_state<R: Runtime>(app: AppHandle<R>) -> OfficeState {
    app.state::<Shell>().state()
}

/// Shows the card beside a desk.
#[tauri::command]
fn card_show<R: Runtime>(app: AppHandle<R>, seat: Option<usize>, interactive: bool, focus: bool) -> Result<(), String> {
    card::show(
        &app,
        card::Show {
            seat,
            interactive,
            focus,
        },
    )
}

/// Hides the card and gives the keyboard back.
#[tauri::command]
fn card_hide<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    card::hide(&app)
}

/// Runs a debug command, dev builds only.
#[tauri::command]
fn debug<R: Runtime>(app: AppHandle<R>, cmd: String) -> Result<String, String> {
    debug::run(&app, &cmd)
}

/// Saves a page's snapshot, dev builds only.
#[tauri::command]
fn snapshot_save(dir: String, name: String, png: String) -> Result<String, String> {
    debug::save_snapshot(&dir, &name, &png)
}

/// Builds the two windows hidden, turns them into panels, places the office and shows it.
fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(tauri::ActivationPolicy::Accessory)?;

    let (work, _) = shell::work_area(app).ok_or("no primary monitor")?;
    let frame = layout::office_frame(work, app.state::<Shell>().desks());
    let office = WebviewWindowBuilder::new(app, OFFICE, WebviewUrl::App("index.html".into()))
        .title("Budd office")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .focusable(false)
        .focused(false)
        .accept_first_mouse(true)
        .visible(false)
        .inner_size(frame.w, frame.h)
        .position(frame.x, frame.y)
        .build()?;
    let card = WebviewWindowBuilder::new(app, CARD, WebviewUrl::App("card.html".into()))
        .title("Budd card")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .accept_first_mouse(true)
        .visible(false)
        .inner_size(card::WIDTH, card::HEIGHT)
        .position(frame.x - card::WIDTH, frame.y)
        .build()?;
    native::install_key_monitor();
    native::convert(&office, native::Kind::Office)?;
    native::convert(&card, native::Kind::Card)?;
    office.set_ignore_cursor_events(true)?;
    card.set_ignore_cursor_events(true)?;
    shell::place(app);
    native::order_front(&office, false)?;
    shell::run_pointer_loop(app.clone());
    Ok(())
}

/// Runs the app. `budd-app --debug <command>` while it runs relays the command to it and exits.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if let Some(command) = debug::command_in(&args) {
                match debug::run(app, &command) {
                    Ok(reply) => log::info!("debug {command}: {reply}"),
                    Err(error) => log::warn!("debug {command}: {error}"),
                }
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(if cfg!(debug_assertions) {
                    log::LevelFilter::Debug
                } else {
                    log::LevelFilter::Info
                })
                .targets([Target::new(TargetKind::Stdout), Target::new(TargetKind::Webview)])
                .build(),
        )
        .manage(Shell::new())
        .invoke_handler(tauri::generate_handler![
            office_toggle,
            office_state,
            card_show,
            card_hide,
            debug,
            snapshot_save
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            setup(&handle)?;
            if let Some(command) = debug::command_in(&std::env::args().collect::<Vec<_>>()) {
                let reply = debug::run(&handle, &command);
                log::info!("debug {command}: {reply:?}");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Budd");
}
