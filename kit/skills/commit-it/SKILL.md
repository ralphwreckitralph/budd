---
name: commit-it
description: Write the commit and PR body in the house format (Summary - Problem / Root Cause / Solution / Testing / Compatibility / Risk Assessment / Notes / Verification) from the real diff, tests, and review findings, then commit, push the branch, and open the PR without waiting for approval, offer the tracker's Root Cause dropdown on a Bug ticket, and end with the links. Requires a sealed /budd:finalize-it record - `budd guard` hard-blocks the commit and the PR without one. Trigger when the human wants a commit message, a PR description/body, "write the commit", "write the PR", "prepare the PR writeup", "commit and open the PR", or invokes /budd:commit-it. State facts, never assumptions. Never merges.
---

# commit-it: house-format commit + PR writeup

Produce the commit message and PR body for the current work.
Everything must be grounded in the ACTUAL `git diff`, the tests that ran, and any review findings.
State facts, not assumptions. If a fact is unknown, ask or omit; never invent.

## Written in the make-it-easy style

Every word this skill writes is written in the `/budd:make-it-easy` style: the commit message, the PR body, the handover close, and the terminal replies.
The rules are in `skills/make-it-easy/SKILL.md` and are not repeated here.
Short version: one idea per sentence, direct verbs, every project term glossed in one clause on first use, repetition and filler removed, never a fact, a number, a path, a warning or a rule.
Written that way from the first line, never as a second pass over a finished draft.
Before anything is committed, run the skill's contract check: every fact, number, path and identifier the source has is present in what you wrote.
Terminal replies inside this skill keep the `/budd:insimpleterms` shape and take the sentence rules on top.

What that means here:

- **The commit skeleton stays fixed**: Conventional Commits type, imperative subject, ticket ref. The body paragraph and bullets follow the sentence rules. No opening block, no glossary: a commit is read in a log.
- **The PR body section names stay fixed** (Problem, Root Cause, Solution, Testing, Compatibility / Parity, Risk Assessment, Notes, Verification) because a CI reviewer and people read them by name. Above the `## Problem` heading, add a `Read this first` block of three to seven lines and a short `Words in this PR` list glossing every project term the body uses. The first sentence under each fixed heading carries that section's claim.
- **The Verification table stays a table.** Marks and one-line reasons, no prose around them.

## Rules

0. **A finalize record is required, and it is a HARD BLOCK, not a preference.** This skill may not commit or open a PR in a product repo until `/budd:finalize-it` has sealed a record for the exact tree being shipped. `budd guard` enforces it on `git commit` (including the `git -C <repo> commit` form used below) and on `gh pr create`, so a missing or stale record fails as a tool error rather than as a bad PR. Before drafting anything, confirm it:

   ```bash
   budd finalize check --repo <anchor worktree> --explain
   ```

   If it refuses, STOP and run `/budd:finalize-it` (or hand the human what it said). Do not work around it, do not reach for another tool that commits, and never write the `I_CONSENT=finalize-override` token yourself: that token is the human accepting unverified work, and only they may type it. The record is also the SOURCE for several sections below: the Verification table comes from its gates, Risk Assessment from its findings and its destructive items, "was anything left unfixed" from its review statuses, and Problem / Solution from the sidecar it names, when there is one. Read it (`budd finalize status --repo <anchor worktree>`) instead of retyping numbers from memory.

1. **Resolve the ticket without asking when the session already names it.** If the invocation, the branch name, the worktree, or the work just done identifies the ticket, use it and say which one you used. Ask ONLY when nothing in the session identifies it, and never guess a ticket id from a plausible-looking number. With tracker `none` (`ticket.kind` on the project card), the key is the `BRIEF-<date>-<slug>` the run was given, and the PR has no ticket link.
2. **No em-dash characters** anywhere in the output. Use commas, colons, periods, or parentheses.
3. **Facts only.** Pull the file list from `git diff --stat`, the fixes from the actual hunks, the evidence from the test suites, the risks from the code-review findings. Do not describe changes you cannot see in the diff.
4. **Conventional Commits**: `fix:` / `feat:` / `chore:` / `refactor:` / `test:` / `docs:`, lowercase, imperative, concise title, ticket referenced.
5. **No approval gate: commit, push, and open the PR, then report the links.** Do not stop to have the draft signed off. Write the message and body, then carry the work all the way to an open PR and end with the links (see "Ship it" below). The writeup is not the deliverable, the PR is.
6. **Three things this skill still never does.** It never merges (`gh pr merge` is blocked by `budd guard` anyway), it never pushes to a branch in `git.protected` (push the ticket branch and open a PR into the base), and it never commits a never-commit file. Before committing, list what is staged and confirm it excludes the profile's `git.never_commit` set (`budd profile get git.never_commit` prints it): local config, env overrides, local test harnesses. Stage the intended paths explicitly. Never `git add -A` or `git add .`.
7. **Stop and report instead of proceeding** if the lint or test gates are not green, if the diff contains something you cannot explain, or if a push or PR command fails. A red gate is the one case where the no-approval rule does not apply: say what is red and hand it back.

## Gather the facts (run these first)

- The **acceptance sidecar**, when the record names one (`budd finalize status --repo <anchor>` prints it, and `record.json` carries `.proposal`). It holds the items the ticket actually raised with the ids the PR review will quote.
- The **expectation document**, when the worktree's handover log has a `do-it` section (`budd handover show --repo <anchor worktree>`). Read the LAST `expectations` section: write **Problem** from its `Problems` bullets, **Solution** from its `Expected` bullets, each checked against the diff, and put its `Potential risk` line in **Risk Assessment**. On a bug, write **Root Cause** from the `cause` section (its `Reported` and `Cause` lines). Write **Testing** from the `E1`..`En` statuses and the test runs in the `built` section, and say plainly that the lane does not reproduce in the app: the failing and passing test runs are the only reproduction, or, when the `Test` line says none, the bug was never reproduced. **Notes** carries every bullet that is NOT MET or NOT CHECKED, by id, and every step the `built` section says a person still has to take (a `.sql` file for the DBA, a server setting). The Summary carries the line "Expectation lane: built from the expectation document with `/budd:do-it`; no proposal page and no approval."
- `git -C <repo> diff --stat @{upstream}...HEAD` (or `<base>...HEAD` / `HEAD`) for the blast radius (files + APIs). Do this per repo when a change spans several.
- The test run for the test COUNTS only. Evidence is the frames in the evidence set, shown on the session's card in Budd, not a hosted or committed report.
- Code-review output (`/code-review`) for Risk Assessment and shared-component blast radius.
- The finalize record (`budd finalize status --repo <anchor>`) for the gate results with their real exit codes, the severity tally, anything shipped unfixed with a human consent behind it, and the destructive items the human cleared. The Verification table is transcribed from its gates: a check is green only because a gate in the record exited 0.

## Commit message

```
<type>: <imperative summary, <= ~70 chars> (TICKET-ID)

<one short paragraph: what changed and why>
- key change 1
- key change 2

Refs: <ticket link>
```

Keep the body to what the diff actually does.
For a multi-repo change, commit each repo with its own scoped message.
Do NOT add any Claude/AI attribution trailer (no `Co-Authored-By: Claude`, no "Generated with" footer) to commits or PR bodies.
With tracker `none`, drop the `(TICKET-ID)` and the `Refs:` line.

## PR body template

Fill every section from facts.
Omit a bullet rather than pad it.
`(Optional)` sections (Compatibility / Parity, Notes) may be dropped entirely when there is nothing factual to say; never leave the literal `(Optional)` placeholder in a real PR body.
The ticket link goes on its own line right under `# Summary` (`budd ticket url <KEY>` prints it; omitted for tracker `none`).

```markdown
# Summary
<ticket link>

## Problem
<briefly describe what was broken / needed and why it matters. If from a bug report, mirror its numbered items.>

## Root Cause
<why did it happen? the underlying cause in the code/config, not the symptom. For a non-bug feature, state what was missing instead.>

## Solution
<what changed? concise, factual, tied to the file/mechanism. No "should"/"probably".>

- <fix, naming the file/mechanism>
- <fix>

## Testing

### Manual
- <what was exercised by hand / in the browser and what it confirmed>

### Automated
- <tests created to simulate the problem: names / what they force>
- <tests that block regressions: names, e.g. one per ticket item>
- <happy + fail paths: what passes on success; what the failure paths assert>

## Compatibility / Parity
(Optional)
- <parity or backward-compat notes, if any>

## Risk Assessment
- Frontend / Backend? <which side(s) changed>
- Shared component? <which, who else consumes them, verification done or flagged>
- API? <routes/contract touched, or none>
- Database? <schema/SP/query touched, or none>
- Breaking change? <yes/no + why>

## Notes
(Optional)
- <anything a reviewer needs that does not fit above>

## Verification

| Check | Status |
|--------|--------|
| Lint | ✅ |
| Unit Tests | ✅ |
| Build | ✅ |
| Standards | ✅ |
| Browser pass | ⏭️ not owed (scope clean) |

Not covered: <every gate that was not configured, off, or opt-in and not run, each with its reason>
```

## Ship it (no approval gate)

Run these in order once the writeup is drafted.
Report the real output of each, never a predicted one.

1. **Confirm the finalize record passes** for every repo you are about to touch (`budd finalize check --repo <anchor> --explain`, rule 0). That check IS the gate confirmation: it re-reads the real exit code of every recorded gate and refuses if the working tree moved since they ran. If it refuses, stop and report per rule 7; a stale record means the gates describe a tree you are no longer shipping.
2. **Stage explicitly and show what is staged.**

```bash
git -C <repo> add <path> <path>          # named paths only, never -A / .
git -C <repo> diff --cached --name-only  # show it, confirm the never-commit set is absent
```

3. **Commit.** Write the message to a file and use `-F`, so the body keeps its line breaks.

```bash
git -C <repo> commit -F <scratchpad>/commitmsg.txt
```

   `budd guard` treats `git commit -a` as committing every unstaged tracked change and blocks on a never-commit file; never use `-a`, and do not stage a never-commit file to make the message go away.
   In a background job use `$CLAUDE_JOB_DIR/tmp` instead of a shared temp dir, one file per repo (`commitmsg_fe.txt`, `prbody_fe.md`), expanded to its absolute path in any `!` line: a shared temp path is reused by parallel jobs and every write there waits on a prompt.

   A staged deletion on a path in `git.test_paths` (`git diff --cached --name-status | grep '^D'`) is refused with no bypass. Check before committing, and hand the human every repo's commit in ONE `!` line joined with `&&`, in a single ask.

   Write and change the commit message and PR body with the Write and Edit tools, never a python or heredoc script: `budd guard` reads words like `UPDATE` in that text as database SQL.

4. **Push the ticket branch** (never the base branch).

```bash
git -C <repo> push -u origin <branch>
```

5. **Open the PR** into the repo's base (`repos[].base`), body from a file.

```bash
gh pr create --repo <owner/repo> --base <base> --head <branch> \
  --title "<commit title>" --body-file <scratchpad>/prbody.md
```

   For a change spanning several repos, commit and push each repo with its own scoped message and open one PR per repo, then cross-link them in both bodies.

   **Called from `/budd:do-it`, or when `pr.draft` is true, add `--draft`.** That lane has no proposal approval, so no person has read the code before this PR; the draft is where they do, and marking it ready is theirs. Never run `gh pr ready` in that case.

5b. **When the PR is already open, update it instead.** A branch may arrive with a DRAFT PR per repo, opened before any human finalized it. Check before step 2, once per repo:

```bash
gh pr list --repo <owner/repo> --head <branch> --state open --json number,url,isDraft
```

   An open PR changes step 5 and nothing else. Rule 0 still applies in full: no commit and no push without the sealed record. Steps 2 and 3 commit whatever the finalize review fixed, and only that; nothing fixed means nothing to commit. Step 4 pushes to the same branch. Step 5 becomes:

```bash
gh pr edit <number> --repo <owner/repo> --body-file <scratchpad>/prbody.md
gh pr ready <number> --repo <owner/repo>      # only when typed by the human outside /budd:do-it and pr.draft is false
```

   The body is rewritten whole from the sealed record, exactly as for a new PR. Never open a second PR for a branch that has one.
   **Called from `/budd:do-it`, skip `gh pr ready`.** Its PRs stay drafts on a rerun too; only the human marks them ready.

6. **Set the tracker's root cause (Bug tickets only), with ONE dropdown.** The PR body's `## Root Cause` section and the ticket's Root Cause field are the same fact, so fill the field while it is fresh instead of leaving it for whoever closes the ticket. Follow `references/root-cause.md`, INTERACTIVE mode, through the project's comment skill (`ticket.comment_skill`), which knows the tracker's fields: read the fields first, skip when it is not a Bug or both levels are already filled (a parent with no child is INCOMPLETE, fill the child), then put the pair you derived from the diff in front of the human as a single `AskUserQuestion` with the recommended option first, set both fields in one call, and read them back. Non-blocking: a refusal or a missing field never holds up the PR, just say the Child is still empty. With no comment skill, or a tracker with no such field, record the intended `Parent -> Child` in the hand-back and move on.
   **Called from `/budd:do-it`, skip this step** and say the Root Cause field is left for the closing comment, so the run is not interrupted by a question that is not critical.

7. **Put what a PERSON must now do where it survives the worktree.** Anything that came up that a person has to do and the document never named goes in the PR body's Notes and in the handover close below. The commonest case is real: `budd guard` hard blocks an agent from staging CI, lint or test config (`git.gate_config`), so a needed change to one of those files cannot ride the PR and has to be committed by a human.

8. **End with the links**, as the last thing in the reply, so they are never buried:

```
PR:     <url>
Commit: <sha> on <branch>
Ticket: <url>
```

   Add the CI status if a check has already reported, and state plainly that the PR is NOT merged and that review is the human's call.
   `Ticket:` is omitted with tracker `none`.

## Close the handover

The PR is the durable artifact, but a handover that stops before the PR is the one that wastes your time next week.
As the last step, once the PR is open:

```bash
budd handover append --repo <anchor worktree> --skill commit-it --title "PR #<N> open" --session <session id> <<'MD'
<body>
MD
```

The `--session` id is this session's id (`CLAUDE_CODE_SESSION_ID` in the Bash tool's environment).
It stamps the section with what the work has cost so far: fresh, cache-write and cache-read tokens in, and output tokens out.
Omit it and the section honestly says `not recorded` rather than carrying a guess.

Three things is enough:

- The PR url, one per repo when several moved, and the branch each was pushed to.
- What is deliberately NOT in the PR that was discussed, so a reviewer's question next week has an answer sitting on disk.
- What is still owed: the CI result, a follow-up already agreed, a closing comment not yet posted.

Then print the `budd handover show --repo <anchor worktree>` line in your closing message, so the human knows where the whole story is.

## Section guidance

- **Problem**: the problem, not the fix. Short critical paragraph, then the concrete issues as bullets. If from a bug report, mirror its numbered items.
- **Root Cause**: the underlying cause in the code/config, not the symptom. For a pure feature with no bug, state what capability was missing instead.
- **Solution**: what you did, tied to files/mechanisms, no "should"/"probably". A series of fixes as bullets. Concise.
- **Testing**: the point is proof, not claims. Split Manual (what you exercised by hand / in the browser) from Automated. Under Automated, cite the specific tests, including the ones that SIMULATE the failure (a forced 500, an abort) and the regression pins. Never claim a test passed without having run it. Cite the counts ("N/N passing"); the evidence frames are on the session's card in Budd. Do NOT host, commit, attach, or link a test report on the PR.
- **Compatibility / Parity** (optional): parity or backward-compat notes. Drop the section if there is nothing factual to say.
- **Risk Assessment**: answer each line honestly (Frontend/Backend, Shared component, API, Database, Breaking change). A shared-component change is the highest-risk line: name the OTHER consumers and the guard/test covering each, or flag it for manual verification. Call out anything deferred for a browser/runtime check.
- **Notes** (optional): anything a reviewer needs that does not fit above. Drop if empty.
- **Verification**: the gate summary, transcribed from the record. Mark each check ✅ only if it actually ran green; use ❌ (with a one-line reason) or ⏭️ (skipped, why) otherwise. Never mark a check ✅ you did not run. The `Not covered` line names every gate that did not run, with the reason, so "did not run" is never read as "passed".

## Sharing the evidence (not in the PR)

Evidence is the before/after frames the lane captured, in the evidence set (`budd evidence show <KEY>`), shown on the session's card in Budd.
The PR body states the fix + the test counts and links the PR(s); it does NOT host, commit, attach, or link a test report.
Do NOT commit a generated report into the branch, do NOT zip or attach it to the PR, and do NOT use a drive, a pages site or a gist.

## Worked example (abbreviated)

```markdown
# Summary
<ticket link>

## Problem
The report diverged from its legacy version and had several production defects: filters returned no data, Export to CSV was dead, and the calendar reset misbehaved.

- The ID filter had no values
- Export to CSV did nothing when data was shown
- Month-to-Date / Last-Month presets were inaccurate; Reset left the Agent populated

## Root Cause
The report's requests went through the legacy HTTP instance, which carries no token, so the API rejected the filtered queries. Export was bound to a dead handler, and the shared date component's preset math computed the wrong range boundaries.

## Solution
Brought the report to parity and fixed the reported defects at the source.

- Auth: route the report through the shared `api` instance so the request carries the token
- Export: wire the button to `generateCSVContent` (was a dead handler)
- DateSelect: correct MTD/Last-Month math; AgentSelect gains a `v-model`

## Testing

### Manual
- Drove the report in the browser: dropdowns populate, tables render, CSV downloads on click, Reset clears the Agent

### Automated
- Simulated failures: a forced HTTP 500 and a network abort assert a graceful error toast
- Regression pins: one test per ticket item (#2..#6)
- Automated: 61/61 passing; frames in the evidence set

## Compatibility / Parity
- Matches the legacy report's behavior for filters, export, and date presets

## Risk Assessment
- Frontend / Backend? Both (report + auth wiring; backend SQL builder)
- Shared component? DateSelect + AgentSelect (consumed by 5-6 reports); preset math unit-tested, AgentSelect binding flagged for manual verification on 4 other screens
- API? No route/contract change; only which HTTP instance sends the token
- Database? the report's SQL builder (tz, subquery, level filter); unit-tested
- Breaking change? No

## Verification

| Check | Status |
|--------|--------|
| Lint | ✅ |
| Unit Tests | ✅ |
| Build | ✅ |
| Standards | ✅ |
| Browser pass | ✅ ui-readonly, 2 surfaces |

Not covered: Playwright e2e (opt-in, not asked); diff coverage (no coverage block in the profile)
```
