# Compliance

Budd runs the user's own, unmodified Claude Code binary under the user's own Claude subscription.

## What Budd never does

- Modify, patch, wrap, relocate or bundle the Claude Code binary.
- Open Anthropic's login flow itself, read a device code, or attach a terminal to the login.
- Read, copy, back up or delete `.credentials.json`, Keychain items, or any OAuth field in `.claude.json`.
- Route a request through a proxy, a base URL, an API key or a token of its own.
- Pass `--dangerously-skip-permissions` or run crew in bypass mode.

## Commercial Terms

Anthropic's legal page states that preinstalling or running Claude Code in a product requires agreeing to the Commercial Terms of Service.
Acknowledged by the maintainer on: (date to be recorded).

## Questions sent to Anthropic

Sent through https://www.anthropic.com/contact-sales on: (date to be recorded). Reply: (to be recorded).

1. Budd is a free, open-source desktop app that launches the end user's own, unmodified Claude Code binary with `claude --bg` under a separate `CLAUDE_CONFIG_DIR`, after the user signs in with `claude auth login` in a visible terminal. Is this within "an end user signing in to the unmodified Claude Code binary with their own Claude subscription"?
2. Budd registers hooks, an MCP server and `permissions.defaultMode: auto` in that separate config folder, and reads `claude agents --json` and hook payloads. Does that count as a modification or an intermediation?
3. Budd automates session lifecycle: a keep-alive restarts an idle session, an autopilot mode answers a question the session asked with the session's own recommended answer, and a timer sends the user a notice when a session needs them. Is there any limit on automated prompting under subscription plans?
4. Does the "plain text" permission cover the sentence "Budd runs the Claude Code you installed" in the README and store listing, and does the name "Budd" with a robot icon raise any trademark concern?
5. Is any agreement beyond the Commercial Terms needed for an open-source distribution, and should the app carry a notice?
