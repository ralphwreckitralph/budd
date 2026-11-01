---
name: learn-it
description: Learn from one finished session and fix the kit so the next run goes better. Reads the session through `budd digest` (never the raw transcript), finds what cost the human time, round trips or context, traces each cost to the skill, agent or reference text that caused it, and proposes ONE lesson branch on the kit repository with at most three small lessons, held by `budd learn scope` to prose only, never a guardrail. Causes in the binary, the hooks, the profile or a product repo go in the lesson's note for a human. Started by Budd's daemon after each /budd:* crew session finishes, or by the human typing /budd:learn-it <session id or name>.
disable-model-invocation: true
---

# learn-it - every run teaches the next one

Before this, lessons were found by hand, by reading a finished run.
This skill does that reading after every ticket session, and turns what it finds into a lesson branch the human reviews in Budd (Review, Apply, Discard).

It never merges, never touches a product repo, never answers another session, and never changes a gate.
The human applying the branch is the only approval a lesson gets.

## Step 1 - read the session

The argument is a session id, a short id, or a crew name (the ticket key in lower case).

```
budd digest <session>
```

Read the digest whole; it is a few kB.
Do not read the transcript itself; when one row needs more, `budd digest <session> --row <n>` prints that row's tool input and result, and nothing else.
Also read the ticket session's job line (`detail`, `needs` in the digest header) and anything the human typed.

## Step 2 - what was learned already

Read the ledger, `state/learnings.jsonl` on the kit's `mine` branch (`$CLAUDE_PLUGIN_ROOT/state/learnings.jsonl`), one JSON object per lesson:

```
{"at":"2026-10-09","session":"b80eaf88","ticket":"PROJ-1055","key":"vpn-down-blocks-pr","kind":"fix","problem":"...","cause":"skills/finalize-it/SKILL.md:361","branch":"learn/proj-1055"}
```

`kind` is `fix` (changed here), `human` (needs a change this skill may not make) or `note` (a decision the human made).
Also list open lessons: `budd learn list` prints every `learn/*` branch not yet applied or discarded, with its lessons.

- A cost whose `key` has an OPEN lesson branch: do not propose it again; count it in the note ("seen again in <ticket>").
- A cost whose `fix` was applied and shows up again: first check it was live. Sessions load skills from the kit's checked-out `mine` branch, so compare the lesson's apply date with the session's start; a session that started before the apply ran on the old text. Not live: count it as seen again, not failed. Live: the fix did not work; propose a stronger one under the same key.
- A `note` is the human's decision. Report what it cost this run, never reverse it.

## Step 3 - find the lessons

Go through the digest sections in this order of cost, and for each row ask whether a different instruction would have avoided it:

1. Waits on the human: could the ask have come earlier, in one message instead of several, or not at all?
2. Consents and approvals the human gave: could the step have written to a place that needs none?
3. Hook blocks and failed calls: was the command wrong, or the instruction that produced it?
4. Repeated commands and reads: which instruction made the session do it twice?
5. Context: the baseline before any work, skill prompts loaded whole for a lane that uses a part, the largest tool results, subagent answers longer than the parent needs.
6. The hand-back: did the human have to ask for status, or chase something the session could have finished?

For each cost, open the file you believe caused it and read the lines.
A cause you did not read is a guess; leave it out.
Check the project's memory for a decision behind it before calling it a mistake.

Sort each one:

- **fix**: the cause is prose in the kit: a `skills/*/SKILL.md` or a skill's `.md` card, `agents/*.md`, `references/**/*.md`, `docs/`.
- **human**: the cause is the `budd` binary, a hook, the profile, a setting, a test, the coding standard, the finalize seal, Claude Code itself, or a product repo. Write the exact change for the note; do not make it.
- **note**: the cause is a choice the human made on purpose.

## Step 4 - choose at most three fixes

Rank the fixes by what they cost the human (waits and round trips first, then wall time, then context), and keep the top three.
Each fix is the smallest change that removes that cost.
Prefer cutting, moving or sharpening text over adding it: a longer prompt is a cost every later run pays.
Never weaken a gate, a consent, or a rule the human set, even when it caused the cost; that is a `human` item.

No fix worth making? Skip to Step 7 and say so.

## Step 5 - make the change on its own branch

```
budd learn propose --name <name> --start
```

`<name>` is the ticket session's name (the key in lower case).
That creates the branch `learn/<name>` from the kit's `mine` branch in a temporary work tree, prints its path, and never touches the live kit at `$CLAUDE_PLUGIN_ROOT`.
Edit only inside that work tree, by absolute path, and run git there with `git -C <work tree>`.
Never call `EnterWorktree`: it stops for an approval, and this run is unattended.
Append one ledger line per lesson, fixes and `human` and `note` items alike, with `branch` set to `learn/<name>`.

## Step 6 - the gates

Both run in the work tree, and both must pass before anything is committed:

```
budd learn scope --repo <work tree> --base mine
budd kit verify --repo <work tree>
```

`budd learn scope` holds a lesson to prose and the ledger, at most six files and sixty lines of net growth, and fails when a skill loses its typed-start line (`disable-model-invocation: true`).
`budd kit verify` checks the frontmatter still parses and no file carries a conflict marker.
When either fails, shrink the lesson or move it to `human`. There is no override.

The rethinker gate fires on this skill (`config/rethinker.json`, class `learn`): before the branch is proposed, spawn the `rethinker` on the lessons, fix each lesson a point shows is wrong or too big, drop a lesson whose cause it could not confirm, and list in the note what changed.
A RETHINK verdict on a lesson drops that lesson.
Never ask the human anything here; nobody is there.

## Step 7 - propose the branch

Commit with a conventional message (`learn(<skill>): <what the next run does differently>`) in the work tree, then:

```
budd learn propose --name <name> --note <scratch file>
```

That pushes the commit onto `learn/<name>` in the kit repo, attaches the note, and shows the lesson in Budd with Review, Apply (merge into `mine` plus `budd kit verify`) and Discard.
Nothing is merged by this skill.

The note, one sentence per line:

- `Learned from <ticket> (<session short id>), <wall time>, peak context <n>k.`
- A table of the fixes: what it cost (with the digest row as evidence), the cause (`file:line`), the change.
- `Needs a human`: each `human` item with its exact change.
- `Decisions that cost time`: each `note` item and what it cost this run.
- `Seen again`: open lessons this run hit again.

If there was nothing to fix, propose no branch; end with one line saying why, plus any `human` and `note` items, and append those to the ledger through `budd learn propose --name <name> --ledger-only`.

## Step 8 - hand back

A few lines: the branch name, the one lesson that saves the most, how many items need a human.
Applying is the human's, in Budd; the temporary work tree is removed by `budd learn propose` once the branch is pushed.
A lesson is live only once applied into `mine`: say so, every time.
