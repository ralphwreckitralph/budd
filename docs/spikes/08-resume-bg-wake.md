# Spike 8: `claude --resume <session id> --bg` as a way to wake a retired session

Date: 2026-11-01. Claude Code 2.1.316 on macOS. The spike 0 session was stopped with `claude stop`, then `claude --resume <its session id> --bg --model haiku "Reply with the single word ping."` was run from the same folder.

## Result: not a wake path

- A new session started with a new short id and a new session id.
- Its SessionStart hook reported `source: "fork"`; its `session_title` kept the old name, `spike0`.
- The old session's `SessionEnd` fired at the stop; the old id never reappeared in `claude agents --json`.

So `--resume --bg` forks the conversation into a fresh session rather than waking the retired one. The engine keeps today's wake path: `claude attach <short id>` in a pty, then wait until `claude agents --json` lists the session with a pid and `idle`, then paste.
