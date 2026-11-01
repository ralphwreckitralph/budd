# The pipeline: from a ticket key to a draft PR

Budd ships a pipeline for working a ticket with Claude Code.
You type a ticket key into Budd's Start Tickets box; a session reads the ticket, writes down what the change should make true, builds it with a small team of agents, checks the work through a set of gates, and opens a draft pull request for you to read.
This page says how a ticket flows, what the gates are, and exactly where it needs you.

## Before the first ticket

- Budd runs Claude Code under its own folder (`~/.budd/claude/`) with the pipeline installed as a plugin at `~/.budd/pipeline/budd/`. Your own `~/.claude` is untouched.
- Each project has a profile: its tracker (Jira, GitHub Issues, or none), its repositories, how a worktree is made, and the lint, test and build commands that prove a change. Budd writes it from the app, or `budd profile init` detects it from the repo's manifest; a ten-line profile is enough for a single repository. The full schema is `kit/references/profile.md`.
- Optional: coding standard cards (the pipeline grades the diff against them), a worktree script that starts the app's servers, and the evidence-shots module (headless screenshots before and after).

## How a ticket flows

```
Start Tickets  ->  read the ticket  ->  worktree  ->  find the code  ->  the document
     ->  BEFORE frames  ->  team build  ->  coding standard  ->  finalize (gates, seal)
     ->  commit, draft PR  ->  AFTER frames, CI  ->  hand-back  ->  you
```

1. **Start.** You type the key (or a brief, when the project has no tracker). Budd starts a session named after it whose first message is `/budd:do-it <KEY>`. Nothing else can start this lane: not the model, not a subagent, not another session.
2. **Read the ticket.** The summary, the description and every comment, quoted verbatim. A ticket too thin to write one expectation from stops here and asks you to add detail.
3. **Worktree.** A fresh worktree per ticket from the project's base branch, and its servers started if the profile knows how.
4. **Find the code.** The session checks whether the base branch already fixed it, reads the real code and every other reader of what it will change, records the cause of a bug, and decides whether the change is **critical** (backend, database, auth, money, server settings, anything irreversible) or **UI/UX only**. A critical change gets a second, independent read by the rethinker agent; a UI/UX-only change skips it to save time. If a browser pass will be owed, it asks you to log in now, in a named tab, so the login never waits at the end.
5. **The document.** Three headings, one sentence per bullet: **Problems** (what is wrong today), **Expected** (what will be true when done, each bullet checkable), **Potential risk** (the one thing most likely to break). Every call a reviewer could make differently is an Expected bullet, so you see it before any code exists. A shape check refuses anything else. The document is shown to you and frozen: the build answers to it, and the hand-back reports each bullet MET, NOT MET or NOT CHECKED by id.
6. **BEFORE frames.** While the worktree is still the base, headless screenshots of the cells each bullet is about, shown on the session's card under Evidence. Skipped when the shots module is not installed; the manifest is kept either way.
7. **Team build.** Named subagents: a test author who writes tests from the document alone, one builder per repository, and a read-only verifier. Each writes only its own paths, checked after every return. On a bug the test is written first and must FAIL on today's code; a test that passes means the bug is already gone, and the run stops with that evidence instead of changing anything. Two builders share the API contract through one file, never by talking to each other. No database writes, ever: a schema change is a `.sql` file for the DBA.
8. **Coding standard.** When the project has cards, a reviewer agent grades the whole diff and returns PASS or a list of fixes, at most three rounds. Without cards the gate is reported as "not configured".
9. **Finalize.** The last gate before anything leaves the machine, as `/budd:finalize-it`:
   - a code review at CI severities, plus a scan against DRY, SOLID, KISS and YAGNI;
   - a destructive scan of the added lines (writes, schema changes, money, deletes, outward sends, auth, secrets), each item put to you as a typed consent;
   - the gates on their real exit codes: lint, unit, build, standards, any project extra gates, run twice when a suite is known to be flaky;
   - a read-only browser pass, only when the change touches surface a break would be invisible on otherwise, with the figures re-checked by a script;
   - four closing questions answered with evidence, every Expected bullet named by id;
   - the seal: a record carrying all of that plus a fingerprint of the exact tree. Any edit after the seal invalidates it.
10. **Commit and draft PR.** `/budd:commit-it` writes the commit and the PR body from the record (Problem, Root Cause, Solution, Testing, Risk Assessment, a Verification table where a check is green only because a gate exited 0, and a "Not covered" line), pushes the ticket branch, and opens a DRAFT PR per repository. A commit or PR without a sealed record is refused by a hook.
11. **AFTER frames and CI.** The same screenshot recipes replayed on the tree that ships, and CI watched for up to 20 minutes.
12. **Hand-back.** The document with each bullet's status and evidence, the files changed, the tests with their failing and passing runs, the draft PR links, what a person still has to do, findings outside the document, a tested link to the screen on the worktree's servers, the evidence paths, and a draft of the closing comment for the ticket, not posted.

## The gates, and what each one refuses

| Gate | Refuses |
| --- | --- |
| Typed start | the lane starting from anything but you typing the command |
| Shape check | an expectation document outside the fixed shape |
| Failing-first test | a bug fix whose test already passes on today's code |
| Ownership check | a teammate writing outside its owned paths |
| Coding standard | shipping without a PASS verdict on the exact tree (when the project has cards) |
| Destructive gate | mail, database writes and resets, recursive deletes, force pushes, remote deletions, container wipes, worktree removal, payment writes; clears only with a consent you type |
| Guard | pushes to protected branches, merges, database writes by any route, staging never-commit files, weakening CI or test config, deleting tests, and any commit or PR without a sealed record |
| Seal | a red or flaky gate, an unresolved blocking finding, a destructive item without your consent and a rollback, an owed browser pass that did not happen, an Expected bullet with no evidence, a "no" among the closing answers |
| Context budget | a session growing past the hard stop; the way out is `/budd:reset-session`, which hands the work to a fresh session |

Advice, not gates: the rethinker and `/budd:grill-me`. They shape a decision and clear nothing.

## What needs you, and nothing else does

Budd shows each of these as buttons on the session's card, so an answer is one click.

| Moment | What you do | The buttons |
| --- | --- | --- |
| A browser pass is owed | log in to the named tab, then say so | `logged in` / `skip` |
| The destructive scan found something | approve each item, one at a time, or stop | `Approve` (types the consent command) / `stop` |
| The run stopped | read why; type the command again with a steer, or add detail to the ticket | none |
| The draft PR is open | read it on GitHub and mark it ready yourself; the pipeline never does | none |
| The closing comment is drafted | post it to the ticket, or skip | `post` / `skip` |
| The PRs are ready to merge | merge in the order the hand-back gives, then say so | `merged` / `holding` |

Three things never happen without you: a consent token (`I_CONSENT=...`) is only ever typed by you, and a token the model wrote is refused and logged; a PR is never marked ready and never merged by the pipeline; a database is never written.

Autopilot, when you switch it on, answers only `Decide:` lines: choices any Claude with the session's context could make, such as which of two equally valid names. It never answers a `Reply:` line, a gate, or a permission prompt.

## After the ticket

- `/budd:learn-it` runs later, unattended, on the finished session: it reads a digest (never the transcript), finds what cost you time, and proposes at most three prose fixes to the pipeline's own skills as a branch you review in Budd (Review, Apply, Discard). It can never change a gate.
- A defect that gets past the seal is logged with the phase that should have caught it, so the pipeline's leaks are measured, not assumed.

## Where things live

| What | Where |
| --- | --- |
| The pipeline (editable) | `~/.budd/pipeline/budd/` (branches `shipped` and `mine`) |
| Profiles | `~/.budd/profiles/<name>.json`, optionally `.claude/budd-project.json` in a repo |
| The finalize record | `<anchor worktree>/.git/budd-finalize/` (dies with the worktree) |
| The handover log | the anchor worktree's git dir (`budd handover show`) |
| Evidence frames | `~/.budd/claude/evidence/<KEY>/`, shown on the session's card |
| Consent log | `~/.budd/state/consent.log` |
| Lessons and escapes | `~/.budd/pipeline/budd/state/` |
