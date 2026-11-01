# Budd

Budd is an office for your Claude Code background sessions: a drawer on the edge of your screen with a desk for each session, where you message them, answer what they ask, start work on tickets, and watch them get on with it.

Budd runs the Claude Code you installed, on your own Claude subscription, in a folder of its own.
It never sees your login, never modifies Claude Code, and never routes a request through anything but your own `claude`.

It is being rewritten for macOS and Windows from the macOS-only original.
Nothing here is usable yet.

## Layout

| Folder | What |
| --- | --- |
| `crates/` | The Rust workspace: the `budd` binary and its library crates |
| `app/` | The Tauri office |
| `kit/` | The bundled pipeline: skills, agents, hooks, references |
| `docs/` | How Budd uses your subscription, Windows setup, troubleshooting, compliance |

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
