---
name: finalize-it
description: The last gate before work leaves the machine. Reviews the change at the same severities a CI reviewer uses (CRITICAL-BLOCKER / MEDIUM / NON-BLOCKING), runs lint + unit + build and gates on their real exit codes, scans the diff for destructive surface and puts anything it finds in front of the human as a typed consent on a Reply line, drives a READ-ONLY browser pass when (and only when) the change touches critical surface a person has to see render, answers the four closing questions, then seals a machine-checkable finalize record. /budd:commit-it is HARD BLOCKED by `budd guard` until that record exists and still matches the working tree. Trigger when the human invokes /budd:finalize-it, or says "finalize this", "is this ready to commit", "final check before the PR", "run the guardrails".
---

# finalize-it - the guardrail over the guardrails

Everything else in the lane decides whether the work is any good.
This decides whether it is allowed to leave.

The difference matters because every other gate is a sentence a model wrote.
`/budd:finalize-it` ends with a **record**: a file in the anchor repo's own git dir carrying the real exit code of every gate, the review findings and how each was resolved, the destructive surface and the human consent that cleared it, whether the change touched surface a person has to see render and the read-only browser pass that proves it does, the four closing answers, and a fingerprint of the exact working tree all of that was true for.
`budd guard` reads that record before it lets a `git commit` or a `gh pr create` through in a product repo.
No record, no commit. A record that no longer matches the tree, no commit.

It reviews, it verifies, it asks. It never commits, never pushes, never opens a PR, never merges.
That is `/budd:commit-it`, and it runs after this, not instead of it.

## What it produces

```
<anchor worktree>/.git/budd-finalize/
  gates/<label>.json    one per gate, carrying the REAL exit code
  logs/<label>.log      that gate's output
  review.json           the findings + how each was resolved
  destructive.json      the scan + the human consents that cleared it
  browser.json          whether critical surface changed, and so whether a UI pass is owed
  chrome-e2e.json       the task-scoped Chrome pass: what it drove, and the frames it kept
  frames/               the pictures that pass left behind
  answers.json          the four closing questions, answered with evidence
  proposal.json         the acceptance sidecar ({ticket, acceptance:[{id,text}]}) this was planned from, when there is one
  record.json           written by `seal`, read by `budd guard`
```

Everything above dies with the worktree.
What outlives it: the handover section of Phase 8 (in the anchor's git dir, read by `/budd:commit-it` and `/budd:reset-session`), the PR body's Verification table and Risk Assessment, which `/budd:commit-it` transcribes from the record, the evidence set (`budd evidence`, outside every worktree), and the consent log (`~/.budd/state/consent.log`).

One artifact deliberately lives outside that directory, because the record dies with the worktree and this must not:

```
<kit>/state/escaped.jsonl    one line per finding that got PAST a seal
```

Every file above proves the gate RAN. That one is the only thing that measures whether it WORKS, so Phase 7 ends by asking you to write to it when something slips through later.

Four subcommands do the mechanical half.
`budd finalize` records and seals; `budd scan destructive` reads the diff for destructive surface; `budd browser` decides whether the change needs a read-only browser pass and then checks that the pass that happened was real; `budd chrome` scopes a Chrome end-to-end pass to the views THIS ticket edits and checks it left frames behind.
All four refuse rather than guess, and a broken internal check fails closed.
Their refusals are the load-bearing behaviour, so they have their own suites in `budd kit test`.

Set these once and use them throughout:

```bash
ANCHOR=<anchor worktree>    # absolute: gates run from the worktree, and a scan item's key hashes the
OTHER=<second repo worktree> # --repo spelling, so every re-run, yours or the human's, uses these exact
BASE=<the anchor's base>     # strings. Omit OTHER if the change is one repo. BASE comes from repos[].base
```

`budd worktree paths <key>` prints all three.

## Phase 0a - resolve the worktree when you were given a ticket key

`/budd:finalize-it PROJ-954` may be typed from any terminal, including one that is in no worktree at all.
Resolve it before Phase 0, and say how you did:

1. Lower-case the key. That is the branch and the worktree name `budd worktree create` would have used.
2. `budd worktree paths <key>` finds it: use it, and load its env (`worktree.env_file`) from the same output.
3. It does not exist: `budd worktree create <key> --track`, which fetches origin and creates the worktree on a LOCAL branch tracking `origin/<key>`. Bring the servers up only when Phase 5 needs them: `budd worktree up <key>`. If the branch is on no remote, stop: nothing was built for this key.
4. Nothing else changes. A branch that arrives with a DRAFT PR already open carries, in its body, whatever gates its author ran. None of that is a record: every gate runs again here, on this tree, and the PR is updated by `/budd:commit-it` after the seal, never by this skill.

## Phase 0 - know what you are finalizing

Establish, and say out loud, before anything else:

- **the repos** the change spans, and which is the anchor (`repos[]` on the project card).
- **the ticket**, if there is one. Resolve it from the branch, the worktree, or the session. Never guess an id.
- **the base**, each repo's `base`.
- **what changed**: `git -C "$ANCHOR" diff --stat "$BASE"` plus untracked files, per repo. If nothing changed, stop and say so.
- **the acceptance sidecar**, when one exists for the ticket: `expectations.json` in the handover directory (`budd handover home --repo "$ANCHOR"`), one item per Expected bullet, `E1` to `En`, written by `/budd:do-it`. Phase 6 will not let you answer "did we satisfy the ticket" without naming evidence for every one of them. Pass that file as `--proposal`. Work with no sidecar (a hand-made branch) runs Phase 6 without it, and the record says so.

Then clear any state from an earlier attempt, so nothing green from a previous tree survives into this one:

```bash
budd finalize reset --repo "$ANCHOR"
```

### Phase 0b - the smoke pass, before you spend anything

Run lint and unit ONCE, unrecorded, before the review: the `verify.<repo>.lint` and `verify.<repo>.unit` commands the project card names, in each touched repo.

This is not a gate and it does not go in the record.
It is here because everything in Phases 1 to 3 describes the tree it ran against: a lint error or a red unit test surfacing at Phase 4 invalidates a full high-effort review, a destructive scan, and a browser pass you already paid for.
Twenty seconds here buys that back.

If it is red, fix it now and then start Phase 1.
If it is green, carry on; the recorded gates in Phase 4 still run last, over the final tree, because the record must describe what ships and not what you started with.
Inside a `/budd:do-it` run this phase is skipped: Step 6 already ran them on this tree.

## Phase 1 - review, at the severities CI uses

Review the full change against `references/review-checklist.md` in the Senior Developer Review Order, covering all 14 categories.
Use `/code-review` at effort `high` as the correctness engine and cover the remaining categories on top of it.

**Pass the repo path to `/code-review` explicitly, once per repo, and say which repo each run covered.**
Do not rely on the working directory.
`/code-review` resolves its diff from where it is invoked, and the finalize target is by definition NOT the project root, so a run started at the root reviews the root: it produces a confident, well-formed review of entirely the wrong code, and nothing in the output looks wrong.

```
/code-review  high <anchor worktree> --base <BASE>
/code-review  high <other worktree> --base <BASE>
```

Before each run, confirm you are pointed at the right tree:

```bash
git -C "$ANCHOR" rev-parse --show-toplevel   # must be the worktree, not the project root
git -C "$ANCHOR" diff --stat "$BASE"         # must show the change you are finalizing
```

The base has to resolve inside the worktree too (`git -C "$ANCHOR" rev-parse --verify "$BASE"`).
If it does not, use `origin/<base>`.
A base that does not resolve produces an empty diff, and an empty diff reviews clean.
`seal` cross-checks this later by refusing findings whose files exist in none of the repos being finalized, but that is a backstop, not a substitute for looking.

Tag every finding `[CRITICAL-BLOCKER]`, `[MEDIUM]` or `[NON-BLOCKING]`, using the same rules as the CI reviewer that will look at this PR, when the project has one.
The point of matching is that a finding CI would block on must be found HERE, before the PR exists, not after.

Every finding carries a **confidence** (High / Medium / Low) and a **citation** that resolves on disk (`file:line`, a lint rule, a failing test id).
A finding with no evidence path is a question, not a defect.

An extra gate the profile lists under `verify.<repo>.extra_gates` with `when: always` belongs to this review in spirit when it reads across layers (a request-key contract, a schema join): anything it reports is a finding here, not a note.
Run such gates early if the change touches what they read.

### The principle scan

The 14 categories ask whether the change is correct.
The four principle cards in `references/software-principles/` ask whether it is built the way this codebase is built, which is the half a green test suite cannot see.
Run all four over the diff, every time, and say which one produced each finding:

- **DRY** (`dry.md`): grep each new function, composable or component name for a near-identical sibling, and enumerate the readers of every figure the diff touches. A report figure with a second reader in an export where only one side changed is the defect class this catches.
- **SOLID** (`solid.md`): SQL inside a request handler, a response built inside a repository, a view calling the raw HTTP client instead of the shared instance, an interface grown a method nobody calls.
- **KISS** (`kiss.md`): a flag, option or abstraction with one call site and no ticket line behind it, and any save path that writes something the screen never mentions.
- **YAGNI** (`yagni.md`): a changed file, added field or added column that no acceptance item, defect or gate explains.

Each card ends with its own severity mapping, and it maps onto the same three tags as everything else here, so a principle finding is resolved exactly like any other: fixed, or taken to the human.

Two rules keep this from turning into a sermon:

- **A principle finding names a consequence.** "This duplicates the total computation, so the CSV and the table can disagree" is a defect. "This is not very DRY" is an opinion, and an opinion with no evidence path is a question, not a finding.
- **A pre-existing violation the diff did not touch is not a finding against this change.** Report it and leave it. The tree already carries known duplicates, and re-opening them inside somebody's bug fix widens a PR that was approved for something else.

State the outcome as one line beside the tally, so a clean scan is a recorded fact rather than a silence: `principles: DRY F3 | SOLID clean | KISS clean | YAGNI F4`.

Then resolve them:

- **CRITICAL-BLOCKER / MEDIUM**: fix it, or take it to the human in Phase 3. Those are the only two outcomes. You may not accept your own blocking finding.
- **NON-BLOCKING**: fix it if it is cheap, otherwise record it as `accepted` with the reason.

Write the result and store it:

```json
{ "decision": "REQUEST CHANGES",
  "findings": [
    {"id":"F1","severity":"CRITICAL-BLOCKER","file":"src/x.php","line":212,
     "summary":"hold % recomputed from merged rows","evidence":"src/x.php:212, tests/HoldTest.php::testMerge",
     "confidence":"High","status":"fixed"},
    {"id":"F2","severity":"MEDIUM","file":"src/y.vue","line":88,
     "summary":"raw http client drops the token","evidence":"src/y.vue:88 vs api.js interceptor",
     "confidence":"High","status":"approved","approvalRef":"consent:<key>"}
  ] }
```

```bash
budd finalize review --repo "$ANCHOR" --file <findings.json>
```

Write `<findings.json>`, and every other JSON file this skill reads, with the Write tool, never a `cat <<` heredoc.
In a background job, Write it under `$CLAUDE_JOB_DIR/tmp` and `cp` it into the record directory: the Write tool may refuse overwriting a file in a shared checkout's `.git` there.
`budd guard` reads a heredoc as part of the Bash command, and an English word such as "rename" next to a schema name is refused as a database write.

It refuses a self-accepted blocking finding, and it refuses an `approved` status whose `approvalRef` is not a consent the human typed for that finding's key (Phase 3).

## Phase 2 - destructive assessment

```bash
budd scan destructive --repo "$ANCHOR" ${OTHER:+--repo "$OTHER"} --base "$BASE" --out <scratchpad>/dscan.json
budd finalize destructive --repo "$ANCHOR" --file <scratchpad>/dscan.json
```

The scan reads added lines (against the merge base, so other people's work on the base is not counted as yours) and reports what a human has to clear: writes that are not provably single-row, schema DDL, money movement, irreversible file operations, outward sends, scheduled jobs, auth changes, gate weakening, literal secrets.
Each item gets a stable `key` and an `ask` (the question the human actually has to answer).

It exits **2** rather than reporting `clean` when the base does not resolve in a repo, or when the scan collected no added lines at all.
Both mean it proved nothing, and a destructive gate that answers "clean" over a diff it never read is worse than no gate.
If you see exit 2, fix the base or the repo list and run it again. Never pass a `clean` verdict forward from a run that errored.

Read every item yourself before showing it to anyone.
A regex cannot tell a careful `WHERE id = ?` from a sloppy one, so resolve what you can: measure a real blast radius with a READ-ONLY query through `budd db read` (`SELECT COUNT(*)`, and the `SUM` of every money-bearing column it could touch) and carry the actual number into Phase 3.
Never write "some rows".
An item you believe is a false positive still goes to the human; you present it as a false positive with the reason, and they clear it. Self-clearing is not available, deliberately.

Also apply the standing rules the scan cannot see:

- An automated or scheduled job needs a settings on/off killswitch AND a hard per-run item cap, and the runner ships before the schedule.
- Agents never EXERCISE a write path. A write the change introduces is a HUMAN test, named as such.

Every item the human later clears also owes a way BACK, recorded at the end of Phase 3.

## Phase 3 - one decision per item, as a typed consent, when there is something to decide

If Phase 1 left a blocking finding you did not fix, or Phase 2 found anything at all, the human decides.
**They decide by typing a consent**, and the consent is the audit trail: every token the human types is checked for provenance by `budd gate` (it has to come from a message the human typed, never from the model, never relayed) and appended to `~/.budd/state/consent.log` with the key it cleared.
No wiki page, no local button server: the Reply line is the ask and Budd renders the consent as an Approve button (`references/reply-contract.md`).

1. Put the whole ask in one message: every item with its `key`, the line it quotes, what you recommend (keep, change, drop, or clear as a false positive), and the measured blast radius, never "some rows". A blocking finding you did not fix is an item too, keyed by its finding id.
2. End the message with the Reply line for the FIRST item, and nothing after it:

   ```
   Reply: `I_CONSENT=finalize-approve budd finalize approve --repo <ANCHOR> --key <key> --note '<your recommendation in the human's words>'` or `stop`
   ```

   One consent per Reply line, because each token clears exactly one key and the log records each.
   `--repo` is the exact `$ANCHOR` spelling the scan ran with: the key hashes it.
3. Wait. The human's click or typing runs the command in this session; `budd finalize approve` refuses a key the scan did not produce, refuses to run without the token, and saves the consent under the record. Then end the next turn on the Reply line for the next item, until every item is cleared or the human says `stop`.
4. `stop` means the human wants changes, or wants to look first: make the changes they name, and start again from Phase 1. The tree changed, so nothing earlier still holds.

**Never write the consent command anywhere but the Reply line.** Not in a code block you run, not in a tool call, not as a suggestion in prose.
A token that does not come from the human is refused and logged as such.

`seal` re-reads every consent before it writes the record and refuses unless each scan key has one.
`check`, which `budd guard` runs on every commit, stays offline and reads the same files.

### Then record the way back

A consent is a human accepting the risk of going FORWARD.
It says nothing about getting back, and reconstructing an undo after a bad deploy is the worst possible time to discover nobody wrote one.
So every cleared item owes one, and `seal` refuses without it:

```bash
budd finalize rollback --repo "$ANCHOR" --key "$K" \
  --how "git revert <sha> in the anchor worktree, then flip settings.exportQueue.enabled to false"
```

Write it for someone who was not here: the revert, the flag, the killswitch, the backup.
`n/a` and one-word answers are refused.
An item that genuinely cannot be undone says so and names what limits the damage instead: `irreversible: capped at 1 tenant per run, and the runner has a settings killswitch`.

**Nothing found in either phase means no ask.** Do not manufacture an item to have something to show; say the scan was clean and move on.

## Phase 4 - the gates, on their real exit codes

Run these LAST, after every fix, so the results describe the tree you are about to ship.
Never trust a summary or a digest for a gate. `budd finalize gate` records the process exit code and exits with it.

**The commands come from the project profile**, the `verify` block the project card summarizes and `budd profile get verify.<repo>` prints in full.
That is the one place the project names its lint and test entry points, so a project that changes how it lints changes it once.
Read it rather than recalling a command.

```bash
budd finalize gate --repo "$ANCHOR" <repo>-lint  --cwd <repo worktree> -- <verify.<repo>.lint>
budd finalize gate --repo "$ANCHOR" <repo>-unit  --cwd <repo worktree> -- <verify.<repo>.unit>
budd finalize gate --repo "$ANCHOR" <repo>-build --cwd <repo worktree> -- <verify.<repo>.build>
budd finalize gate --repo "$ANCHOR" standards-<repo> -- budd standards gate --repo <repo worktree>
budd finalize gate --repo "$ANCHOR" <label> --cwd <repo worktree> -- <each verify.<repo>.extra_gates command>
```

One set per touched repo, recorded in one message.
`budd finalize gate --from-profile --repo "$ANCHOR"` runs exactly that list and is the same thing with less typing; use it when nothing about the tree needs a hand-picked order.
A missing command is recorded as `not configured` and counts against the floor below, except `standards` (see next).

**The `standards` gate is the coding standard made checkable.**
It is green only when `budd standards scan` finds no `auto` violation in the added lines AND the `coding-standards` agent's last verdict is PASS AND that verdict's fingerprint is the tree right now.
So before recording it, spawn the `coding-standards` agent (`agents/coding-standards.md`) with `REPO`, `BASE=origin/<base>`, `SCOPE=whole`, `ROUND`, and `OUT=<scratchpad>/standards-verdict.json`, then record what it wrote:

```bash
budd standards verdict --repo <repo worktree> --file <scratchpad>/standards-verdict.json
```

A REDO comes back as findings with a rule id, a `file:line` and a fix each; fix exactly those, spawn the agent again, and stop at three rounds with the finding in front of the human.
Never write that verdict file yourself and never edit one.
`/budd:do-it` already ran this at Step 7, so on its branch this is usually one round; a branch that skipped it (a hand-made branch) gets its first round here.
**When the project card says `standards: not configured`**, record no standards gate: the floor is lint, unit and build, `seal` accepts that floor only for a project with no `standards.cards`, and the hand-back says "standards: not configured" so the gap is a stated fact.

Plus the task-scoped Chrome pass, whenever the change renders anything (see the next section):

```bash
budd finalize gate --repo "$ANCHOR" e2e-chrome \
  -- budd chrome verify --evidence "$ST/chrome-e2e.json" --surfaces <scratchpad>/chrome-surfaces.json
```

A fresh worktree installs each suite's deps separately; a gate that dies on a missing test runner is a setup step, not a red gate.

### The `e2e-chrome` gate - task-scoped, Chrome only, and it keeps the frames

This one runs on every change that renders anything, and it is cheap.
It drives the views THIS ticket edits, in Claude in Chrome, read-only, and it fails unless the pass left pictures behind.

It exists because of a gap the other gates cannot see between them.
Phase 5's browser pass asks "could this break something a person has to SEE?" and answers no for a cosmetic diff, which is correct for what it guards.
The Playwright gate is opt-in and usually off.
So a caption change, a label fix or a copy edit would finish an entire finalize with **no captured frame anywhere**.
Small tickets are the ones that ship unseen, and they are the ones this catches.

The scope comes from the diff, not from risk:

```bash
budd chrome surfaces --repo "$ANCHOR" ${OTHER:+--repo "$OTHER"} --base "$BASE" --out <scratchpad>/chrome-surfaces.json
```

It lists every changed view file outside tests, docs and the never-commit set, and resolves each to its route through the project's router.
`required: false` means the change renders nothing a browser can be pointed at (a backend-only or test-only diff) and **no pass is owed**: say so and move on.
A base that does not resolve exits **2**, never `required: false`, because an empty file list and a change that renders nothing look identical and only one of them is safe to assume.
A project with no `worktree.app_url` has nothing to drive: `surfaces` prints `required: false` with that reason, and the hand-back says the change was not looked at in a browser.

When it is required, drive each surface and record what you did:

```json
{"driver":"claude-in-chrome",
 "app":"<worktree app url>",
 "session":{"how":"human-login","note":"user logged in as <role>"},
 "surfaces":[{"covers":["<surface id>"],
              "route":"/reports/transactions",
              "frames":["<anchor>/.git/budd-finalize/frames/01-filter-before.png",
                        "<anchor>/.git/budd-finalize/frames/02-filter-after.png"],
              "observed":"caption reads \"Display bonus only:\"; clicking it flips the switch on",
              "verdict":"pass"}],
 "actions":[{"kind":"navigate","detail":"/reports/transactions"},
            {"kind":"toggle","detail":"clicked the caption, switch went off -> on"},
            {"kind":"screenshot","detail":"before and after the click"}],
 "network":[{"method":"POST","url":"<api url>/transactions/list","status":200}],
 "consoleErrors":[],
 "verdict":"pass"}
```

```bash
ST="$(git -C "$ANCHOR" rev-parse --git-dir)/budd-finalize"   # write the evidence where it is kept
budd finalize gate --repo "$ANCHOR" e2e-chrome \
  -- budd chrome verify --evidence "$ST/chrome-e2e.json" --surfaces <scratchpad>/chrome-surfaces.json
```

**Write the evidence file into `$ST`, not a temp path.** The record keeps it, the handover reads its `observed` prose, and a temp path is reused by the next run.

What `verify` refuses, and why each one is a rule rather than a preference:

- **A surface with no frame.** The whole point of this gate. A pass that leaves no picture leaves the reviewer prose, which is the thing it was added to prevent.
- **A frame that is not on disk, is empty, or predates the scan.** A stale capture from an earlier tree cannot stand in for this one.
- **A changed view that nothing covered.** Editing a view and driving a different page is not a pass.
- **Any action outside the read-only allowlist** (`navigate read screenshot filter search paginate sort expand hover scroll console network login toggle`). This is an allowlist on purpose: a list of forbidden verbs is one you have to keep complete, and the cost of missing one is an agent changing a client's data. `toggle` is on it because flipping a filter switch reads; a save does not.
- **An empty request log.** Chrome asserts nothing by itself, so the requests are the only proof a page was exercised rather than merely opened.
- **Any request that answered 400 or worse**, and any surface the pass itself marked not passing.
- **A `driver` of `playwright`.** That belongs in the `e2e` gate below. This gate is for the human-driven Chrome pass, whose value is the frames and the request log.
- **A hand-written surfaces file.** `verify` checks for the scanner's own marker, so a scope you typed is not a scope.

Three things that are yours, not the subcommand's:

- **Keep the frames somewhere durable.** `<anchor>/.git/budd-finalize/frames/`, captured with `save_to_disk: true`. Not a test runner's output dir, which the next run wipes.
- **Capture the state the ticket is about**, not a wide screenshot of the page. One frame before the interaction and one after is what makes a fix legible later.
- **Read-only still binds absolutely.** A ticket whose surface is a save path drives the read side and records the save as a HUMAN test, cleared through Phase 2. That does not change here.

When the pass is green, tell the human the frames exist and where.

### The Playwright e2e gate is OPT-IN. Do not run Playwright unless asked.

**Never run a Playwright suite as part of a finalize unless the human asks for it in this session, in their own words.**
Not when the ticket has a spec, not when the change touches a screen.
`build` satisfies the floor's third slot on its own, so a finalize seals perfectly well with no e2e gate recorded at all.

The reason is cost against value.
A full browser suite can take a quarter of an hour, the `--repeat 2` rule doubles it, and such suites carry standing failures from stale fixtures that belong to whoever is repairing them rather than to the change in hand.
Sitting through that on every finalize is how a gate starts getting skipped for real.
The unit suites and the read-only browser pass in Phase 5 are the last line of defence; a browser suite CI does not run never was.

When the human DOES ask for it:

- Run it headless and without a debug delay; a suite that throws browser windows onto the human's screen mid-work is a different kind of failure.
- Scope it to what the change can reach when the project has a scoper for that (an `extra_gates` entry that prints the spec list, with `ALL` and a reason when it cannot narrow); otherwise run the whole suite and say so. A scoped gate whose scope nobody recorded is indistinguishable from a scope chosen to dodge a red spec, so paste the scoper's reasoning into the seal's evidence.
- Before blaming the change for a red spec, run that spec against a base checkout and compare. A failure that reproduces on the base is not this branch's, and saying so needs the baseline, not an argument.

```bash
budd finalize gate --repo "$ANCHOR" e2e --repeat 2 --cwd <e2e dir> -- <the suite command> $SPECS
```

Phase 6's `noNewIssues` answer must then say plainly that no e2e ran, so the record never implies coverage it does not have.

The floor, enforced by both `seal` and `check`: a **lint** gate, a **unit** gate, a **build or e2e** gate, and a **standards** gate when the project has cards.
Sealing under that floor is refused.

**`e2e-chrome` does not count towards the third slot**, and `floor_ok` excludes it by name.
It drives a dev server and keeps frames, which is its job; it proves nothing about whether the production build compiles, so accepting it there would let a change seal having never been built.
`build` and the Playwright `e2e` gate are what fill that slot.

### Run the unstable ones twice

Flakiness is a defect in its own right, and a suite that passes on one run and fails on the next has proved nothing.
`--repeat N` runs a gate N times, records every run's real exit code, and marks the gate FLAKY when they disagree.
`seal` and `check` refuse a flaky gate before they even mention red, so re-rolling until it happens to be green does not work:

```bash
budd finalize gate --repo "$ANCHOR" <repo>-unit --repeat 2 --cwd <repo worktree> -- <verify.<repo>.unit>
```

Use it wherever order, shared state or timing can decide the result: any gate the project card lists under `flaky:`, and e2e whenever an e2e gate is being run at all.

### The gate that proves the tests reached the change

A green unit gate proves the suite passed.
It does not prove the suite executed one line of your diff, and a suite that covers none of it is a gate in name only.
`budd coverage diff` joins a coverage report to the added lines and names the ones no test ever ran:

```bash
( cd <repo worktree> && <verify.<repo>.coverage.command> )
budd finalize gate --repo "$ANCHOR" diff-cov-<repo> \
  -- budd coverage diff --repo <repo worktree> --base "$BASE" --report <verify.<repo>.coverage.report> --explain
```

It runs only when the profile has a `coverage` block for the repo.
Without a report it exits **2**, which is "nothing was proved", never "covered"; it is not silently skipped, and the hand-back says the gate is off.

It is honest about its reach: an executed line is not an asserted line.
A test that runs your code and asserts nothing satisfies it.
It closes the cheaper hole, which is code no test reaches at all.

### Project extra gates

`verify.<repo>.extra_gates` holds the gates a project adds beyond the floor, each a label and a command that exits 0 only when green (a request-key contract across layers, a schema join, a bundle size check).
Record each with `budd finalize gate` under its label.
A scoped extra gate should fail only on what the change ADDS: a finding the base already had is `(pre-existing)`, recorded as a NON-BLOCKING review finding that names its own ticket, never silently dropped.
Findings from such a gate are review findings: fix them, or take them to the human in Phase 3 like any other blocking finding.

A red gate is not a finding to write up, it is work to do.
Fix it and re-run that gate.
One lint auto-fix pass is fine; suppressing a check is not, and `budd guard` blocks it anyway.

If a fix here changed a source file, the review and the scans now describe an older tree: re-run Phase 2, re-review the hunks you touched, and re-run Phase 5.

## Phase 5 - the read-only browser pass, on critical surface only

> **Not the same thing as the `e2e-chrome` gate in Phase 4.** That one is scoped to the views this ticket edits and its job is to leave frames behind, so it runs on small changes too. This one is scoped to RISK: it runs when a break would be invisible to every other gate, and it demands claims a script re-checks (figures that add up, a repeated run, a cross-view comparison, a data coherence block). A change can owe both, one, or neither. When it owes both, drive the app once and write two evidence files from the same pass rather than driving it twice.

Lint, units and a build all stay green while a page renders the wrong figure, an empty panel, or a login loop.
That class of defect is only visible in a browser, so this phase looks at it, but only when the change can actually cause it, and only ever by READING.

**Never for every change.** A script decides, not you:

```bash
budd browser scope --repo "$ANCHOR" ${OTHER:+--repo "$OTHER"} --base "$BASE" --out <scratchpad>/uiscope.json
budd finalize browser --repo "$ANCHOR" --file <scratchpad>/uiscope.json
```

It reports `required: true` only when the change touches surface where a break is invisible to the other gates: auth, session and route guards (`auth-surface`, `auth-logic`), what a page renders from its data (`data-render`), what it asks the backend for (`data-fetch`), a figure a person reads and acts on (`figures`), the backend read path behind it (`read-path-sql`), the layer every call goes through (`proxy-route`), or an export that renders the same data a second time (`export-parity`).
The profile's `critical.paths` feed it; styling, docs, tests and comments are not critical, and neither are the never-commit local files, which sit dirty in every worktree and would otherwise make every change look critical.
It reads removed lines as well as added ones, because deleting a binding breaks a page exactly as well as adding a wrong one.

**`required: false` means no browser.** Say the scope was clean and go to Phase 6. Do not manufacture a pass to have something to show.

### When it IS required

The pass is READ-ONLY, without exception.
Navigate, filter, sort, paginate, scroll, read, and screenshot.
You do not click Save, Delete, Update, Approve or anything else that writes.
A write this change introduces is a HUMAN test, named as such in the report and cleared through Phase 2, never something an agent exercises (`references/guardrail.md`, and the standing rule in Phase 2).

1. **Get the app in front of you.** Use this worktree's own `worktree.app_url`, not a shared stack, or you are looking at different code. Confirm the URL with the human.
   **The human types the password.** Ask them to log in, or work in a tab that already holds a session; never type a credential yourself.
   The first ask names the tab: title it `BUDD TEST <app port> - click me`, and tell them to search their tabs for `BUDD TEST <app port>`, log in there and stay on it until you confirm. Check the tab left the login page before you go on.
   An ask that names no way to the tab costs several `logged in` replies.
   Load the Chrome tools in ONE call before this ask. None means the session was started without `--chrome`, and `/clear` keeps the launch flags: say so once, and the seal waits for `/budd:finalize-it` in a session started with `--chrome`.
   The ask ends with ``Reply: `logged in` or `skip` ``.
2. **Work through the scope's items** with the tools from step 1. Each item names its file and, when the router resolves it, candidate routes; a component that no route registers has none, so you name the page that consumes it and say how you knew.
3. **Per surface**: land on it, drive the read-only controls that reach the changed code, and capture a screenshot, `read_network_requests`, and `read_console_messages`. Never resize the window: verify at the default viewport.
4. **Re-invoke before you believe it.** Navigate away, come back, and run the same query again. Sticky filters, once-only guards and stale flags only show up on the second pass.
5. **Judge it like a reviewer**: the figures, the empty state, the alignment and overflow at the default viewport. Something that clearly looks off is a finding, even if the ticket did not mention it.

Then write the evidence and let the verifier, not your own summary, decide:

```json
{"driver":"claude-in-chrome",
 "app":"<worktree app url>",
 "session":{"how":"human-login","note":"user logged in as <role>"},
 "surfaces":[{"covers":["<scope item key>", "..."], "route":"/reports/revenue",
              "screenshot":"/abs/path/revenue.png",
              "pageText":"/abs/path/revenue.page.txt",
              "observed":"Net 1,204,880.55 = Gross 9,655,201.00 - Paid 8,450,320.45, totals row matches the sum of 24 rows",
              "figures":[{"label":"Net","value":"1,204,880.55","formula":"Gross - Paid",
                          "inputs":["9,655,201.00","8,450,320.45"]}],
              "totals":[{"column":"Gross","footer":"9,655,201.00","rowsSum":"9,655,201.00","rows":24}],
              "repeat":{"rows":24,"figures":[{"label":"Net","value":"1,204,880.55"}]},
              "crossView":[{"of":"Gross total","aView":"table","aValue":"9,655,201.00",
                            "bView":"csv","bValue":"9655201.00"}],
              "verdict":"pass"}],
 "actions":[{"kind":"navigate","detail":"/reports/revenue"},
            {"kind":"filter","detail":"tenant <the test tenant>, 2026-08-01 to 2026-08-07"},
            {"kind":"read","detail":"compared the totals row against the page rows"}],
 "network":[{"method":"POST","url":"<api url>/reports/revenue","status":200}],
 "consoleErrors":[],
 "verdict":"pass"}
```

```bash
budd finalize gate --repo "$ANCHOR" ui-readonly \
  -- budd browser verify --evidence "$ST/ui-evidence.json" --scope <scratchpad>/uiscope.json
```

Same rule as the `e2e-chrome` gate: the evidence goes in `$ST`, so the handover can read it.

### The four claims that are checked, not read

`observed` is prose, and prose cannot be wrong in a way a script can see. These can:

- **`figures` and `totals`** are owed by a surface covering a `figures` item.
  `verify` re-does the arithmetic: a `footer` that does not equal `rowsSum` within a cent is the defect, whatever the page looked like.
  Then it checks PROVENANCE: every number quoted, inputs included, must appear in the `pageText` the pass captured with `get_page_text`.
  A figure that is not on the page was not read off the page.
  A footer that disagrees with its own rows renders perfectly, answers 200, and passes lint, units and a build; that is the class this catches.
- **`repeat`** is owed by a surface covering a `figures` or `data-render` item.
  The skill has always said re-invoke before you believe it; this is what checks that you did.
  The second run must return the same row count and restate every figure with the same value.
  A query that renders a different group name on the second run, because the source does not return it reproducibly, is why.
- **`crossView`** is owed by a surface covering an `export-parity` item.
  Two renderings of one figure, named and compared.
  A table that formats money while the CSV writes the raw API value is the case.
  Comparing a rendering with itself is refused, since it compares nothing.

The tolerance is one cent, so `9,655,201.00` and `9655201.0` agree and a hundred does not.
`playwright` is exempt from the `pageText` requirement only: the spec IS the assertion there.

`verify` fails the gate, and so the seal, when an action falls outside the read-only set, when a request reached a write-named endpoint, when a request failed, when no request was recorded at all (a page that never called the backend was not exercised), when a critical item is covered by no surface, when a screenshot is missing or predates the scope scan, or when the pass marked anything failing.
`seal` then refuses outright if `required` is true and no green `ui-readonly` gate exists.

A failing surface is work to do, not a note to write up. Fix it, and come back through Phase 1.

### Data coherence, when the change touches a filter or a read path

A page that renders is not a page whose data is coherent, and this phase used to accept the first as proof of the second.

The case that made this section: scoping a dropdown to the selected parent removed the only option that had any rows for that parent, because scoping went through a junction table that had no row for it.
The filter became guaranteed to return nothing.
Nothing threw, every request answered 200, the panel drew correctly, seven gates went green, the record sealed, and the human found it in a minute of clicking.
The pass had proved liveness: a request fired and the header changed.

So for any scope item of kind `data-fetch`, `data-render`, `read-path-sql`, `figures` or `proxy-route`, each covering surface owes a `data` block, and `verify` refuses without one.

**The invariant that catches this class needs no judgement:**

> Every distinct value of the filtered column PRESENT in the broad result must be OFFERED by the filter control.

And the rule that makes an empty result decidable rather than arguable:

> Take the value you narrow by FROM the broad result, never from the dropdown.

Picked from a row you just saw, an empty result is provably a bug.
Picked from the control, empty might just be a value with no data, which is exactly how the above hid.

```json
"data": {
  "broadRows": 222,
  "narrowedRows": 222,
  "narrowedBy": "Outlet 0 - <name> (taken from the broad result)",
  "offeredValuesMissing": [],
  "zeroReason": "only when narrowedRows is 0: why, with a read-only count from the source"
}
```

`verify` refuses a block that contradicts itself, so the shape cannot be satisfied by writing zeroes into it: `narrowedRows` above `broadRows` (a narrowing filter that widened the result), a narrowing to zero with no `zeroReason`, an unnamed `narrowedBy`, or a non-empty `offeredValuesMissing`.

Two things worth doing by hand while you are there, because they are the same failure at a different altitude: check the totals row equals the sum of the page rows, and if a result is empty, confirm WHY with a read-only `SELECT ... GROUP BY` over the source table through `budd db read` rather than assuming.
If the root cause turns out to be the data or a stored procedure, it is reported and handed over, never fixed here.

### Without a browser to drive

`driver` says what did the looking, and the two are anchored differently because they prove different things.
`claude-in-chrome` asserts nothing, so its request log is the only evidence the page fetched at all, and `verify` demands one.
`playwright` IS the assertion: a spec that ran green under the `e2e` gate has already proved what rendered, so it names its `spec` file instead and may record no network.
That is the path a headless run takes, and it is a real read-only pass, not a lighter one.

What is NOT available is skipping.
If the scope says `required` and neither driver can be produced, stop at `NEEDS_HUMAN` rather than sealing, and end on ``Reply: `logged in` or `skip` `` with the tab named; `skip` means the human accepts that the seal waits for a session that can drive the browser.
"We could not look" and "there was nothing to look at" are different facts, and the record must not be able to state the second when the first is true.

## Phase 6 - the four closing questions

Answer these last, in writing, each backed by evidence you can point at.
They are the questions a reviewer asks before letting a change out, so they are a gate, not a retrospective.

1. **Did we satisfy the ticket?** Walk every acceptance item and name the evidence that closes it, **by id**. An item with no evidence means the answer is no.
2. **Did we find all the issues and fix them?** Every finding from Phase 1, its severity, and its outcome. Anything found and NOT fixed is named here with the consent that cleared leaving it.
3. **Did we introduce issues by fixing this issue?** The blast radius of the fix itself: what else consumes the code you changed, what proves it still works, and what you could not prove.
4. **Is this ready for committing and a PR?** Gates green, destructive surface cleared, the browser pass done or provably not owed, never-commit set absent from what will be staged.

```json
{"ticketSatisfied":{"verdict":"yes","evidence":"E1..E3 each closed by tests/<ticket>-*.spec.ts; E4 by the ui-readonly frame 02"},
 "allIssuesFound":{"verdict":"yes","evidence":"F1 CRITICAL fixed at src/x.php:212; F2 MEDIUM cleared by consent <key>; F3 NON-BLOCKING accepted"},
 "noNewIssues":{"verdict":"yes","evidence":"DateSelect is shared by 5 reports; the unit suite covers the preset math, the chrome pass covers 2 consumers, the other 3 flagged for manual check"},
 "readyToShip":{"verdict":"yes","evidence":"6 gates exit 0 including ui-readonly; scan clean; the never-commit set excluded from the staged set"}}
```

```bash
budd finalize answers --repo "$ANCHOR" --file <answers.json> \
  --proposal "$(budd handover home --repo "$ANCHOR")/expectations.json"
```

A single `no` refuses the seal, and that is correct: a `no` is the human's decision to make, not something to seal around.
Thin evidence is refused too. "yep" is not an answer.

**`--proposal` is what makes question 1 a gate instead of a sentence.**
Without it, "did we satisfy the ticket" is the one closing answer nothing can check, because no other artifact in the record knows what the ticket asked for.
With the sidecar, every acceptance item it committed to must be named in the `ticketSatisfied` evidence, so an item that quietly stopped being built fails here rather than in staging.
Pass it whenever the work has one; `seal` and `check` re-verify it afterwards, and once a finalize has a sidecar it cannot be re-answered without one.
Work with no sidecar behind it (a hand-made branch, a hotfix) omits the flag and the record says so.

## Phase 7 - seal, and hand off

```bash
budd finalize seal --repo "$ANCHOR" ${OTHER:+--also "$OTHER"} --ticket <TICKET> --base "$BASE"
budd finalize check --repo "$ANCHOR" --explain
```

`seal` re-verifies everything before it writes: no flaky gate, every gate green, the floor present, the `ui-readonly` gate present when the browser scope said this change needs one, no unresolved blocking finding, every scan item cleared by a consent the human typed AND carrying a recorded rollback, every sidecar item evidenced, all four answers yes.
It then stamps each repo with a fingerprint of its whole working tree, tracked and untracked, plus the base's tip sha and the list of files this change touches.

That fingerprint is why the record cannot drift.
It is identical before and after `git commit`, so one seal covers the commit, the push, and the PR.
It changes the moment any file changes, so a single edit after sealing sends you back through the gates rather than shipping something nothing verified.

### Where the record reaches other people

Everything `seal` just wrote lives in `.git/budd-finalize/`, which `git worktree remove` destroys, on a machine nobody else has.
The kit does not publish a page; what reaches a reviewer is:

- the PR body `/budd:commit-it` writes from the record: the Verification table from the gates (a check is green only because a gate exited 0), the Risk Assessment from the findings and the cleared items, and a `Not covered` line naming every gate that was not configured, off, or opt-in and not run, so the body distinguishes "did not run" from "not applicable",
- the handover section of Phase 8, in the anchor's git dir,
- the evidence set and its frames, on the session's card in Budd,
- the consent log, one line per cleared item.

Then report to the human, briefly: the review tally, the gate results with real counts, what the scan found and what they cleared, the four answers, and that the work is now committable.
Hand off to `/budd:commit-it`. Do not commit here.

### And when something gets through anyway

It will.
When the PR's CI review, staging, or a person finds a defect this gate should have caught, log it.
One line, and it names the phase that owns the miss:

```bash
budd finalize escaped add --ticket <TICKET> --source ci-review \
  --severity CRITICAL-BLOCKER --file src/x.php --line 212 --phase 1 \
  --what "the footer summed TotalAmount while the rows rendered Amount" \
  --why  "the review read the query and never compared the rendered totals"
```

`budd finalize escaped report` then answers the only question that says whether any of this is working:

```
by phase that should have caught it:
    phase 1: 4
    phase 5: 1
```

Every other artifact in the record proves the gate RAN.
This is the one that shows where it LEAKS, and the phase at the top of that list is the check to sharpen next.
Naming a phase is required, because an escape with no owner teaches nothing.
The ledger is `state/escaped.jsonl` in the kit repo, so it survives every worktree and travels with the kit's `mine` branch.

## Phase 8 - write the handover

The record proves the gates ran.
It does not carry what you decided, and in a week that is the half you will want back.

Append a section as the last act of a seal, after `seal` has written `record.json`, so the `seal` fact in the block carries the real fingerprint rather than `unsealed`:

```bash
budd handover append --repo "$ANCHOR" --skill finalize-it --title "sealed at <fingerprint>" --session <session id> <<'MD'
<body>
MD
```

The `--session` id is this session's id (`CLAUDE_CODE_SESSION_ID` in the Bash tool's environment).
It stamps the section with what the work has cost so far: fresh, cache-write and cache-read tokens in, and output tokens out.
Omit it and the section honestly says `not recorded` rather than carrying a guess.

Four things, briefly:

- **The gate results** as counts that actually ran: how many gates, how many green, and any `--repeat` disagreement.
- **Every blocking finding and how it was resolved**, plus every NON-BLOCKING one accepted and the reason. `review.json` holds the data; the handover holds the judgement.
- **The destructive surface**, if the scan found any, and the consent that cleared each item.
- **What the browser pass showed**, when one was owed.

A seal that FAILED gets a section too, titled with whatever refused.
"We stopped here, and this is why" is worth more next week than silence.

## The hardblock

`budd guard` refuses `git commit` and `gh pr create` in any repo the profile lists under `repos[]` unless `budd finalize check` passes for it.
It covers `git -C <dir> commit` too, which is the form `/budd:commit-it` uses.
`gh pr create` run from outside a repo is refused outright, because the record cannot be located from there.

It also refuses a record whose BASE moved out from under it.
The fingerprint proves the tree did not change; it says nothing about the base gaining commits in the same files while the PR waited.
When that happens, `check` names the overlapping files and sends you back: the gates ran against a merge that no longer exists.
A base that advanced elsewhere prints a note and passes, and your own commit on the base is not drift at all.

The block clears in exactly two ways:

1. Run `/budd:finalize-it` and seal a record for the tree you are shipping.
2. The **human** types `I_CONSENT=finalize-override <command>`.

Never write that token yourself.
Every use of it is the human saying they accept shipping work that nothing verified, and it is theirs to say (`references/guardrail.md`).
When you are blocked, print what `check` said, state what is missing, and either go do it or hand the human the exact command on a Reply line.

**`finalize-override` covers this block and nothing else.**
The other `budd guard` rules have NO consent slug at all, by design: deleting a test or spec file, staging CI, lint, type or test config (`git.gate_config`), and adding a suppression directive are "Hard guardrail, no bypass".
There is no `I_CONSENT=` form that clears them, so never offer the human one.
If a test genuinely has to go (a duplicate, a spec for deleted code), say plainly that the deletion has to be done by the human outside the hooked tooling, and then re-run the gates over the result.

Committing in the kit repo itself and in a repo the profile does not list is not gated by the seal. Product repos and their worktrees are.

## Written in the make-it-easy style

Every word this skill writes is written in the `/budd:make-it-easy` style: the handover it writes in Phase 8, the `answers.json` prose, the findings in `review.json`, the undo notes on cleared items, and the terminal replies.
The rules are in `skills/make-it-easy/SKILL.md` and are not repeated here.
Short version: one idea per sentence, direct verbs, every project term glossed in one clause on first use, repetition and filler removed, never a fact, a number, a path, a warning or a rule.
Written that way from the first line, never as a second pass over a finished draft.
Before anything is recorded, run the skill's contract check: every fact, number, path and identifier the source has is present in what you wrote.
Terminal replies inside this skill keep the `/budd:insimpleterms` shape and take the sentence rules on top.

What that means here:

- **The handover is markdown you write**, so it opens with a `Read this first` block of three to seven lines and a short `Words this page uses` list.
- **A finding names the evidence in plain words.** "The e2e gate did not run: `gates/e2e.json` is absent" beats a paragraph about exit codes.

## Rules

- **The record describes the tree, not the intention.** Re-run the gates after any fix, however small. The fingerprint will catch you either way.
- **A flaky gate is a red gate.** Never re-run a suite until it happens to be green. `--repeat` makes the disagreement a recorded fact, and both `seal` and `check` refuse it.
- **Figures are checked, not read.** State them as the page showed them, capture the page text they came from, and let `verify` do the arithmetic. Prose in `observed` proves nothing.
- **Every cleared destructive item owes an undo**, written for someone who was not here.
- **Log what escapes.** A defect the CI review or staging finds later goes in the ledger with the phase that should have caught it. A gate nobody measures drifts into theatre.
- **Never seal over a red gate**, and never make a gate green by weakening it. Both are refused mechanically, in `seal` and in `budd guard`.
- **You do not clear your own destructive surface.** The human does, by typing a consent `budd gate` can trace to them, one per item, and `budd finalize approve` records it. You never write the token.
- **The browser pass is read-only, and only for critical surface.** A script decides whether it is owed; you never decide you have done enough of one. Reading a page is yours, writing to it is the human's.
- **A change that renders something leaves a picture behind.** The `e2e-chrome` gate is scoped to the views the ticket edits, so "it was only a caption" is not an exemption; that is the case it was added for. Keep the frames in `.git/budd-finalize/frames/`, and tell the human they are there. In a `/budd:do-it` run the BEFORE and AFTER frames per expectation live in the evidence set instead (`budd evidence`), outside the worktree.
- **Facts only, with citations.** Every finding, every answer, every blast radius names the evidence. If the evidence to conclude is missing, say so instead of guessing.
- **This skill never commits, pushes, opens a PR, or merges.**
- **The record covers the code, never the per-environment config.** The fingerprint respects `.gitignore`, so every never-commit local file is OUTSIDE it. That is deliberate (they sit dirty in every worktree and would make each seal unique), but it means a change whose behaviour is decided per environment is not verified by anything here. Say so in the Phase 6 answers and carry it as a human todo; no gate will catch it.
- **It is honest about its own reach.** Diff coverage proves a line was EXECUTED, not asserted. The figure checks prove the numbers you quoted are on the page and add up, not that the page's own arithmetic matches the business rule. The destructive scan is regexes over added lines: multi-line SQL and a write assembled at runtime slip past. The browser scope is regexes over the diff: it can call a change critical that is not (cheap, you do one pass) and it can miss one that is, and `verify` can prove a screenshot was taken and postdates the scan but not that anyone looked at it. The record is files this agent can write. Neither is proof against a determined agent; both make every claim a separate artifact that has to line up, and make drift mechanical to detect. Say that plainly rather than describing the record as a guarantee.
