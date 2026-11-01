# Spike 2: the per-session inbox socket as a send path

Date: 2026-11-01. Claude Code 2.1.316 on macOS, against the spike 0 session while it sat idle.

## Wire format

Claude Code's own help text (in the binary) shows it: newline-delimited JSON over the Unix socket named by `CLAUDE_CODE_MESSAGING_SOCKET`, an `auth` line first, then a `user` message.

```
{"type":"auth","token":"<CLAUDE_CODE_MESSAGING_TOKEN>"}
{"type":"user","message":{"role":"user","content":"Reply with the single word pang."}}
```

The server answered nothing on the connection within 6 seconds.

## What happened

- The message was delivered at once, with no hold dialog, because the child token marks it as coming from the session's own hook or Bash command.
- `UserPromptSubmit` fired with `prompt` equal to the clean text and a fresh `prompt_id`; `Stop` fired with the same `prompt_id` and `last_assistant_message: "pang"`.
- The transcript recorded it as a `user` entry with `isMeta: true` whose content begins "Another Claude session sent a message:", followed by the text and a paragraph telling the model it was not typed by the user and can never count as approval for a pending prompt. Two `queue-operation` entries with no content preceded it.
- The model answered the request anyway ("pang").

## Verdict

Usable as a transport, unusable as the human's voice. The pty paste stays the primary and only path for answers, consent and `/clear`. The inbox may serve plain, non-consent chat later, with the engine marking such sends so the reader pairs them with their replies by `prompt_id` rather than by transcript text.

Also noted in the binary's strings: cross-session messaging sits behind a remotely toggled feature gate ("cross-session messaging gate off"), so the socket may be absent on some accounts. The socket path rule when the env var is missing: `$XDG_RUNTIME_DIR` or the temp dir, then `cc-socks/<pid>.sock`, with `/tmp/cc-socks-<uid>/` when the path is too long.
