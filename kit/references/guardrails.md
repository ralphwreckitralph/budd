# The guardrails, and how they are managed

One table of every hard guardrail the kit ships: what it stops, where it is wired, and how it is proved to still work.
`budd kit test` runs every suite and exits non-zero on any failure; `budd kit verify` checks the wiring of the live kit.

## The inventory

| Guardrail | Stops | Wired where | Its test |
| --- | --- | --- | --- |
| `budd gate` | mail, database writes and resets, recursive deletes, `git reset --hard`, force push, remote branch or PR deletion, container volume wipes, worktree removal, payment writes, fan-out edits; the `finalize-override` and `finalize-approve` consents (`references/guardrail.md`) | `PreToolUse` on `Bash`, `Edit`, `MultiEdit`, `Write` and `mcp__.*`, through `budd hook PreToolUse` | the gate suite in `budd kit test` (every slug, its negative case, and the provenance check) |
| `budd guard` | `gh pr merge`; pushing a branch in `git.protected`; any database change by any route (mutating SQL in a command, a write verb from `http.write_verbs` against the project's API, running a script or `.sql` file that mutates, authoring mutating SQL outside `database.authoring_allowed`); staging or committing a path in `git.never_commit`; weakening the verification gate (a path in `git.gate_config`, a suppression directive, deleting a file under `git.test_paths`); `git commit` and `gh pr create` in a product repo without a sealed finalize record (escape hatch `I_CONSENT=finalize-override`, typed only by the human) | `PreToolUse` on `Bash` and on `Write`, `Edit`, `MultiEdit`, through `budd hook PreToolUse` | the guard suite in `budd kit test` |
| `budd db read` | any write through the agents' read tool: one statement starting SELECT, SHOW, DESCRIBE, DESC, EXPLAIN or WITH, no comments, no `INTO OUTFILE`; run read-only and rolled back; a 120 s limit. `budd guard` counts it as database context, so a mutating statement handed to it is refused twice | the read path the team briefs name; the command behind it is `database.read` | the db-read suite in `budd kit test` |
| `budd lint-edit` | a written product file that breaks a coding-standard `auto` rule, or the repo's `verify.*.file_lint` | `PostToolUse` on `Edit`, `Write`, `MultiEdit`, through `budd hook PostToolUse` | the standards suite in `budd kit test` |
| `budd standards gate` | shipping without a PASS verdict written by the `coding-standards` agent for the exact tree, when the profile has `standards.cards`; "not configured" when it has none, which the seal and the hand-back say out loud | called by `budd finalize gate` as the `standards` gate | the standards suite in `budd kit test` |
| `budd finalize seal` | a commit without green lint, unit, build and standards gates (the floor); unresolved CRITICAL or MEDIUM findings; destructive items not cleared by a typed consent; missing rollbacks; a flaky gate; an owed browser pass with no green `ui-readonly` gate; an acceptance item with no evidence; a `no` among the four closing answers; base drift into changed files | called by `/budd:finalize-it`; `budd guard` refuses the commit without its record | the finalize suite in `budd kit test` |
| `budd finalize approve` | clearing a destructive item without the human: it runs only under `I_CONSENT=finalize-approve`, which `budd gate` checks against the transcript, and it refuses a key the scan did not produce | `/budd:finalize-it` Phase 3, through the Reply line | the finalize suite in `budd kit test` |
| `budd expect check` | a `/budd:do-it` expectation document outside the fixed shape: a missing, repeated or out-of-order heading, a bullet with more than one sentence or over 300 characters, more than one risk bullet, an em dash | Steps 4 and 5 of the skill, before the document is shown and logged; exit 0 is the only pass, and only a passing document gets the sidecar (`E1` to `En`) that `budd finalize answers --proposal` checks by id | the expect suite in `budd kit test` |
| `budd team check` | a teammate writing outside its owned paths: it holds `git status` of every repo against the `team.roles` globs of the teammates spawned so far | after every teammate return in `/budd:do-it` Step 6 | the team suite in `budd kit test` |
| `budd learn scope` | a `/budd:learn-it` lesson changing anything but prose: a path outside `skills/**/*.md`, `agents/*.md`, `references/**/*.md`, `docs/` and the ledger `state/learnings.jsonl`; any hook template, config, script or this inventory; a skill losing `disable-model-invocation: true`; more than 6 files; more than 60 lines of net growth | Step 6 of `/budd:learn-it`, before the commit; exit 0 is the only pass, no override flag | the learn suite in `budd kit test` |
| `/budd:learn-it` typed start | a lesson started inside a ticket session by the model, where it would fill that session's context | `disable-model-invocation: true` in its `SKILL.md`; Budd's daemon starts it as its own crew session | `budd kit verify` (frontmatter check) |
| `/budd:do-it` typed start | a build with no proposal and no approval started by anything but the human typing the command: the model through the Skill tool, a subagent, a request relayed from another session | `disable-model-invocation: true` in its `SKILL.md` | `budd kit verify` (frontmatter check) |
| `/budd:do-it` draft-only PRs | the lane's run to a PR leaving it ready for review, when no person has read the code | Step 10 of the skill and the `/budd:do-it` paragraph of `/budd:commit-it` step 5 (`gh pr create --draft`, never `gh pr ready`) | the commit-it text check in `budd kit verify` |
| `budd budget` | a main session growing past the hard stop (`kit.contextBudget` in Budd's settings, default 150,000 tokens): a warning from the warn line (default 130,000; after a tool once per 5,000 tokens, and on every prompt), then every tool call denied until the human types `/budd:reset-session`, which unlocks the tools for that one turn so the handover brief can be written; arming the brief, or any other prompt, locks them again. Subagents are out of scope, and it fails open on its own errors because it is a cost guard, not a safety guard | `PreToolUse` (unmatched entry), `PostToolUse` and `UserPromptSubmit`, through `budd hook` | the budget suite in `budd kit test`; the handover marker: the reset suite |
| the kit canary | a session starting with `bin/budd` at the wrong version or `hooks.json` drifted from the template, without anyone noticing | `SessionStart` through `budd hook SessionStart`; prints one line, never blocks | `budd kit verify` |
| `captain` agent `tools:` line | the captain session doing anything but messaging: reading, running, editing, spawning, starting a skill, posting | the agent frontmatter, applied when the human launches `claude --agent budd:captain` | the captain wiring check in `budd kit verify` |

Advice, not gates: the `rethinker` agent and `/budd:grill-me`.
The rethinker trigger (`config/rethinker.json`) keeps `/budd:grill-me` off the ticket chain (`/budd:do-it` through `/budd:commit-it`) and off the unattended `/budd:learn-it`.
In `/budd:do-it` and the finalize and commit it runs, the gate defers to the lane's `surface` line, so a UI/UX-only change skips the rethinker.
They shape a decision; they clear nothing.

## Why one binary, one hook per event

A hook runs only if the settings file that wires it is loaded, and which user settings load depends on the config dir that launched the session.
The kit avoids that whole class of drift: Budd runs every session under its own `CLAUDE_CONFIG_DIR`, the plugin wires one exec-form entry per event, and `budd kit verify` compares the live `hooks.json` with the template.
Turning a gate off is possible only in Budd's settings (`kit.gates`), read by `budd hook` on every call, logged, and shown as a banner while off.

## The rules for changing a guardrail

- A gate is changed in its code and in its test in the same commit. A refusal that silently stops refusing is invisible, so almost every test asserts a rejection.
- A new gate gets a row in the table above and a suite in `budd kit test` together.
- `budd kit test` is green before a kit release. A red suite is fixed, never worked around.
- No agent ever writes an approval, a finalize verdict, a consent token, or the standards verdict. Those stay with the human or with the one subcommand that owns them.
- A lesson from `/budd:learn-it` may change prose only; `budd learn scope` holds it there.
