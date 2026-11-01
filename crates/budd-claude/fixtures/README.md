# Recorded Claude Code surfaces

One folder per Claude Code version. Everything here was captured from real sessions and sanitized: home paths become `/Users/alice`, scratch paths are shortened, tokens are dropped.

- `hooks/`: one file per hook event, with the `CLAUDE_*` environment (minus tokens) and the stdin JSON.
- `transcripts/`: session transcripts (`projects/<slug>/<session id>.jsonl`), the unstable format the compat parsers are tested against.

Record a new version with `cargo xtask record-fixtures` once it exists; until then, see `docs/spikes/00-hook-env.md` for the capture recipe.
