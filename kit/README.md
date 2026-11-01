# Budd pipeline kit

The pipeline Budd installs into its own Claude Code folder as a plugin: the ticket-to-draft-PR lane, its gates, the evidence set, the learning loop, the session handover, and the Reply/Decide contract Budd's office reads.
This folder is the plugin.
Installed, it is the git repository at `~/.budd/pipeline/budd/`, loaded in place by Claude Code, and yours to edit.

## What is in it

```
.claude-plugin/plugin.json   the plugin manifest (name "budd")
hooks/hooks.template.json    one exec-form `budd hook <Event>` entry per event; rendered to hooks/hooks.json at install
skills/                      do-it, finalize-it, commit-it, learn-it, reset-session, grill-me, insimpleterms, make-it-easy
agents/                      rethinker, coding-standards, captain
references/                  reply-contract, profile (the schema), project-card, guardrail, guardrails, ticket-flow,
                             deterministic-first, review-checklist, root-cause, software-principles/
config/rethinker.json        which skill classes fire the rethinker gate, and how
examples/profiles/           two profiles that parse: a single GitHub repo, a paired Jira project
state/                       ledgers that outlive a worktree: learnings.jsonl, escaped.jsonl (tracked, merged by union)
bin/                         the budd binary, copied in by install (gitignored)
```

Skills are invoked with the plugin prefix: `/budd:do-it <KEY>`, `/budd:finalize-it`, `/budd:commit-it`, `/budd:reset-session`, `/budd:grill-me`, `/budd:insimpleterms`, `/budd:make-it-easy`, and `/budd:learn-it <session>` (started by Budd's daemon).

## How a project plugs in

Nothing in the kit names a project.
Every project fact comes from a profile (`references/profile.md`): the tracker, the repos, the worktree command, the lint and test commands, the critical paths, the never-commit set.
Budd keeps profiles in `~/.budd/profiles/<name>.json`; a repo may carry `.claude/budd-project.json` as an override.
At `SessionStart`, `budd hook` injects a project card under 2 KB (`references/project-card.md`), and the skills say "the lint command the project card names" instead of a command.
`budd profile get <path>` prints any field in full.

A project with no coding standard cards runs with a three-slot seal floor (lint, unit, build) and says so.
A project with tracker `none` runs from a typed brief.
A project with no servers hands back a file and line instead of a tested link.

## How it is installed

`budd kit install` (run by the app on first launch, or by hand):

1. Creates `~/.budd/pipeline/` as a local marketplace root with the `kit-marketplace` manifest, and `~/.budd/pipeline/budd/` as a git repository holding this tree, committed on branch `shipped` and tagged with the kit version.
2. Creates branch `mine` from `shipped` and checks it out. `mine` is the live tree; it is where your edits go.
3. Copies the app's `budd` binary to `bin/budd` (`bin/budd.exe` on Windows) and renders `hooks/hooks.json` from `hooks/hooks.template.json`, replacing `{{EXE}}`.
4. Runs `claude plugin marketplace add ~/.budd/pipeline` and `claude plugin install budd@budd-kit --scope user` against Budd's own `CLAUDE_CONFIG_DIR`, and merges Budd's keys into that folder's `settings.json` (permissions and env, which a plugin cannot ship).
5. Runs `budd kit verify`.

Edits take effect at the next session start, because the plugin loads in place and nothing is copied.

## How it is updated

`budd kit update` never overwrites your tree.
It verifies the live tree, commits any local edits on `mine`, merges the new `shipped` tree into `mine` in a temporary work tree, verifies that, and only then fast-forwards the live tree.
A conflict leaves the live tree untouched and shows in Budd as "Update <version> waiting: <n> files changed, <m> conflicts" with Open conflicts, Let Claude resolve, Keep mine, Take shipped.
`budd kit diff` shows your edits against `shipped`; `budd kit reset-file <path>` takes one file back to `shipped`; `budd kit status` says which version is live and whether `mine` carries edits.

Invariant: the live plugin dir always passed `budd kit verify`: JSON parses, skill frontmatter is valid, no conflict markers, `hooks.json` matches the template, `bin/budd` is the app's version, no path escapes the root.

## How to edit it

Edit the files under `~/.budd/pipeline/budd/` with any editor, or let a session edit them.
Commit on `mine` when you like; `budd kit update` commits for you if you do not.
A lesson from `/budd:learn-it` arrives as a `learn/<name>` branch, shown in Budd with Review, Apply (merge into `mine` plus verify) and Discard.
Run `budd kit verify` after a hand edit; a session started on a tree that fails it gets one warning line from the kit's canary.

To work on the kit as its author, `budd dev --kit <this folder>` starts Claude Code with `--plugin-dir` pointing here instead of the installed copy.

## What the kit expects from `budd`

Every hook and every script the private lane used is a subcommand of the one `budd` binary.
The skills call these and nothing else; none of them exist yet in this repository.

| Subcommand | One line | Status |
| --- | --- | --- |
| `budd hook <Event>` | The one hook entry point: reads the event JSON, dispatches to gate, guard, budget, rethinker trigger, lint-edit, session-start, state writers | planned |
| `budd gate` | The destructive gate: deny by slug with the `I_CONSENT=<slug>` protocol, provenance-checked against the transcript, logged | planned |
| `budd guard` | Protected refs, `gh pr merge`, database writes by any route, never-commit staging, gate-config weakening, test deletion, commit or PR without a sealed record | planned |
| `budd budget` | The context budget: warn and hard stop per `kit.contextBudget`, unlocked for one turn by `/budd:reset-session`, relocked by `budd reset arm` | planned |
| `budd rethinker-trigger` | Injects the rethinker instruction per the classes in `config/rethinker.json` | planned |
| `budd lint-edit` | Runs `verify.<repo>.file_lint` and the standards `auto` rules on a file just written | planned |
| `budd session-start` | Builds the project card, writes `CLAUDE_ENV_FILE`, loads a reset brief on `clear`, runs the canary | planned |
| `budd state` | Writes the per-session state Budd's office reads | planned |
| `budd ticket read\|comment\|url` | Jira (connector or REST token), GitHub Issues (`gh`), or a typed brief; posts a comment; prints the ticket URL | planned |
| `budd worktree create\|up\|level\|paths` | Runs the profile's worktree commands; levels every repo with its base; prints paths, env and app URL | planned |
| `budd handover append\|home\|show` | The per-worktree handover log in the anchor's git dir | planned |
| `budd expect check\|sidecar` | The expectation document shape check and the `E1..En` acceptance sidecar | planned |
| `budd evidence init\|screen\|proof\|check\|show\|shoot` | The evidence manifest per ticket; `shoot` only with the evidence-shots module | planned |
| `budd team check\|brief` | Ownership check of `git status` against `team.roles`; renders a role's brief from `skills/do-it/team.md` | planned |
| `budd standards scan\|verdict\|gate` | The coding standard's `auto` rules, the agent verdict record, the fingerprint-stamped gate | planned |
| `budd finalize reset\|review\|destructive\|approve\|rollback\|gate\|browser\|answers\|seal\|check\|status\|escaped` | The finalize record, one file per claim, sealed with a tree fingerprint | planned |
| `budd scan destructive` | Added lines against the merge base: writes, DDL, money, irreversible ops, sends, jobs, auth, gate weakening, secrets | planned |
| `budd browser scope\|verify` | Whether a read-only browser pass is owed, and whether the one that happened was real | planned |
| `budd chrome surfaces\|verify` | The task-scoped Chrome pass: changed views to routes, and the frame and request checks | planned |
| `budd coverage diff` | Joins a coverage report to the added lines and names the ones no test ran | planned |
| `budd db read` | One read-only statement through `database.read`, rolled back, 120 s limit | planned |
| `budd digest <session>` | The session digest `/budd:learn-it` reads instead of the transcript | planned |
| `budd learn scope\|propose\|list\|apply\|discard` | The lesson scope gate, the `learn/<name>` branches, and their lifecycle | planned |
| `budd reset facts\|session-facts\|candidates\|arm\|show\|disarm` | The handover brief for `/budd:reset-session` | planned |
| `budd profile init\|get\|show\|validate\|card` | The profile: detect, read a field, print, validate, print the project card | planned |
| `budd fingerprint` | The one definition of the working-tree fingerprint the record and the verdict share | planned |
| `budd kit install\|update\|verify\|status\|diff\|reset-file\|test` | Install, three-way update, verify the live tree, show edits, reset a file, run every suite | planned |
| `budd doctor` | Checks Claude Code, the login, the plugin, the binary version and the hooks | planned |

The design each of these follows is the plan this kit was extracted from; the skills describe the behaviour they rely on and nothing more.

## Rules the kit holds to

- The kit names no project. A project fact that is not in the profile is "not configured", never guessed.
- No skill writes an approval, a consent token, a finalize verdict or a standards verdict.
- `/budd:do-it` and `/budd:learn-it` carry `disable-model-invocation: true`; only the human (or Budd typing for them) starts them.
- Hooks are exec-form calls to `${CLAUDE_PLUGIN_ROOT}/bin/budd`; no shell, no interpreter, nothing Windows cannot run.
- Markdown here keeps each full sentence on its own line, and uses a plain dash, never an em dash.
