# Spike 0: what a hook sees in a background session

Date: 2026-11-01. Claude Code 2.1.316 on macOS. A `claude --bg --name spike0 --model haiku --permission-mode auto` session started from a trusted folder, with capture hooks passed through `--settings`.

## Result: pass

Every hook event of the session ran with this environment:

| Variable | Value seen |
| --- | --- |
| `CLAUDE_PID` | the session's own process id |
| `CLAUDE_CODE_SESSION_ID` | the full session id |
| `CLAUDE_JOB_DIR` | `<config>/jobs/<short id>`, so the short id is its basename |
| `CLAUDE_CONFIG_DIR` | the config folder the session runs under |
| `CLAUDE_PROJECT_DIR` | the cwd |
| `CLAUDE_CODE_MESSAGING_SOCKET` | `/tmp/cc-socks/<pid>.sock` |
| `CLAUDE_CODE_MESSAGING_TOKEN` | the child token for that socket |
| `CLAUDE_CODE_SESSION_ATTENDED` | `0` for a background session |
| `CLAUDE_CODE_CHILD_SESSION` | `1` |
| `CLAUDE_EFFORT` | present from the first tool call on |
| `CLAUDE_ENV_FILE` | only on SessionStart: `<config>/session-env/<session id>/sessionstart-hook-<n>.sh` |

Every event's stdin JSON carries `session_id`, `transcript_path`, `cwd`, `scratchpad_dir`, `hook_event_name`, and `session_title` (the `--name`).
From UserPromptSubmit on it also carries `prompt_id` and `permission_mode`; tool events carry `tool_name`, `tool_input`, `tool_use_id`, and PostToolUse `tool_response` and `duration_ms`; Stop carries `last_assistant_message`, `stop_hook_active`, `background_tasks`, `session_crons`; SessionStart carries `source` and `model`.

UserPromptSubmit fired for the prompt given on the `--bg` command line.

## Consequences for the design

- The hook needs no process-table walk: `CLAUDE_PID` is the session's pid.
- The short id comes from `CLAUDE_JOB_DIR` without asking `claude agents`.
- `prompt_id` is the turn key. The state file carries it, and reply matching compares ids, not prompt text. The 139-character prefix match stays only as a fallback for sessions without it.
- The inbox socket path and token are captured at SessionStart for spike 2.
