# The project card

What `budd hook SessionStart` injects into every session as `additionalContext`, under 2 KB, so a skill can say "the lint command the project card names" and find it without reading the profile.
`budd profile card` prints the same text from any working directory.

## What it contains, in order

1. One line: the profile name, the project root, the tracker kind, and the ticket prefixes.
2. One line per repo: id, path, remote, base, and `anchor` on the anchor.
3. The worktree line: whether `create`, `up` and `status` are configured, the env file name, the app URL pattern.
4. One line per repo under `verify`: the lint, unit and build commands verbatim, the extra gate labels, and `flaky:` with the labels that repeat.
5. The standards line: the cards folder, or `standards: not configured (floor is lint, unit, build)`.
6. The git line: protected branches, the count of never-commit patterns, the count of gate-config patterns, the test paths.
7. The database line: `writes: never`, and whether a read command exists.
8. The critical line: the critical path globs and topics.
9. The team line: the role names and, for each, how many globs it owns.
10. The evidence line: `shots: on|off`.
11. The PR line: `draft: true|false`, `ci wait: <n> min`.
12. One closing line: `Details: budd profile get <path>. Reply contract: ${CLAUDE_PLUGIN_ROOT}/references/reply-contract.md. Skills: /budd:do-it <KEY>, /budd:finalize-it, /budd:commit-it, /budd:reset-session, /budd:grill-me.`

A field that is not configured is printed as `not configured`, so a skill reading the card never has to tell an empty value from a missing one.

## What it never contains

- A secret, a token, a password, or the value of any env variable (`evidence.login_env` names variables, the card prints the names).
- Anything over 2 KB: a long list is cut to the first entries and `(+n more, budd profile get <path>)`.
- The Reply contract text itself, which stays in `references/reply-contract.md`.

## What else the hook does at SessionStart

- Writes `BUDD_PROFILE` (the merged profile path), `BUDD_PROJECT` (the root) and `BUDD_KIT` (`CLAUDE_PLUGIN_ROOT`) to `CLAUDE_ENV_FILE`, for shell convenience only; every `budd` subcommand re-resolves from the working directory and never trusts them.
- With matcher `clear`: reads the reset brief `/budd:reset-session` armed for this process (`pending-<pid>.json` in the anchor's git dir), returns it as `additionalContext`, sets the session title, and consumes the marker.
  It fails open: a missing or stale marker means an ordinary empty session.
- Runs the kit's own canary: it checks `bin/budd` is the app's version and `hooks.json` matches the template, and prints one warning line when either is off.
  It never blocks a session.

## Example

```
Budd project: toy (/Users/me/code/toy), tracker github, prefixes: none (keys like #123)
repo app: . remote owner/toy base main anchor
worktree: create configured, up not configured, status not configured, env file none, app url none
verify app: lint `npm run lint` unit `npx vitest run` build `npm run build` extra: none flaky: none
standards: not configured (floor is lint, unit, build)
git: protected main; never-commit 0 patterns; gate-config 3 patterns; test paths tests/**
database: writes never; read not configured
critical: paths none; topics auth, money, schema, deploy, dependency, irreversible
team: test-author (1 glob), builder (1 glob), verifier (read-only)
evidence: shots off
pr: draft true, ci wait 20 min
Details: budd profile get <path>. Reply contract: <kit>/references/reply-contract.md. Skills: /budd:do-it <KEY>, /budd:finalize-it, /budd:commit-it, /budd:reset-session, /budd:grill-me.
```
