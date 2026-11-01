//! The debug channel: `budd-app --debug <command>` from a terminal reaches the running instance
//! through the single-instance plugin, and the pages can call the `debug` command. Dev builds only.

use tauri::{AppHandle, Manager, Runtime};

use crate::card::{self, Show};
use crate::native;
use crate::shell::{self, CARD, OFFICE};

/// The command after `--debug` in a process's arguments, if any.
pub fn command_in(args: &[String]) -> Option<String> {
    let mut args = args.iter();
    args.find(|a| *a == "--debug")?;
    args.next().cloned()
}

/// Writes a page's PNG snapshot where `snapshot:<dir>` asked, as `<dir>/<name>.png`. Dev builds only.
pub fn save_snapshot(dir: &str, name: &str, png: &str) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("snapshots are only in dev builds".into());
    }
    if name.contains(['/', '\\', '.']) {
        return Err("snapshot name must be a plain word".into());
    }
    let bytes = base64_decode(png)?;
    let path = std::path::Path::new(dir).join(format!("{name}.png"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Standard base64 with padding, which is what `canvas.toDataURL` gives; enough to avoid a crate.
fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let value = |c: u8| -> Result<u32, String> {
        Ok(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("bad base64 byte {c}")),
        })
    };
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.trim_end_matches('=').as_bytes().chunks(4) {
        let mut acc = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            acc |= value(c)? << (18 - 6 * i);
        }
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Ok(out)
}

/// Runs one debug command and says what happened. The commands are the README's table.
pub fn run<R: Runtime>(app: &AppHandle<R>, command: &str) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("debug commands are only in dev builds".into());
    }
    match command {
        "office:open" => Ok(format!("{:?}", shell::set_open(app, true))),
        "office:close" => Ok(format!("{:?}", shell::set_open(app, false))),
        "card:show" => card::show(
            app,
            Show {
                seat: Some(1),
                interactive: true,
                focus: true,
            },
        )
        .map(|()| "card shown".into()),
        "card:peek" => card::show(
            app,
            Show {
                seat: Some(0),
                interactive: false,
                focus: false,
            },
        )
        .map(|()| "card peeked".into()),
        "card:hide" => card::hide(app).map(|()| "card hidden".into()),
        "info" => {
            let mut lines = Vec::new();
            for label in [OFFICE, CARD] {
                let window = app.get_webview_window(label).ok_or("no window")?;
                lines.push(format!("{label}: {}", native::describe(&window)?));
            }
            Ok(lines.join("\n"))
        }
        snapshot if snapshot.starts_with("snapshot:") => {
            let dir = &snapshot["snapshot:".len()..];
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            tauri::Emitter::emit_to(app, OFFICE, "snapshot", dir).map_err(|e| e.to_string())?;
            Ok(format!("office page asked for a snapshot into {dir}"))
        }
        "quit" => {
            app.exit(0);
            Ok("quitting".into())
        }
        other => Err(format!("unknown debug command: {other}")),
    }
}
