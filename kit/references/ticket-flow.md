# The ticket flow, end to end

One ticket, from a key typed into Budd to a draft PR and a closing comment.
This is the chain the kit runs; a project may wrap more around it, but nothing in the kit runs a ticket another way.

```
Start Tickets (Budd) or /budd:do-it <KEY> typed   the human's go-ahead, the only one the lane has
        |
   read the ticket        `budd ticket read`: Jira, GitHub Issues, or a typed brief
        |
   worktree               `budd worktree create` from the profile's command, servers up
        |
   cause and surface      handover sections: what is wrong, and whether the change is critical
        |
   the document           Problems / Expected / Potential risk, shape-checked, rethinker on a critical change
        |
   BEFORE frames          the evidence set, while the worktree is still the base
        |
   team build             test-author, builders, verifier; failing test first on a bug
        |
   coding standard        `budd standards` scan, agent verdict, gate GREEN
        |
/budd:finalize-it         review + gates on real exit codes -> SEALS the record
        |
/budd:commit-it           commit, push, open the DRAFT PR (hard-blocked without that record)
        |
   AFTER frames, CI       the same recipes replayed on the tree that ships; CI watched
        |
   hand-back              the document by id, the PR links, the evidence, the comment draft, `Reply:` lines
        |
   the human              reads the draft PR, marks it ready, merges; `post` sends the comment
        |
/budd:learn-it            later, unattended: one lesson branch from the finished session
```

## Where each step happens

| Step | Runs in | Produces | The next step needs |
| --- | --- | --- | --- |
| Start Tickets | Budd's office | a crew session named after the key, whose first message is `/budd:do-it <KEY>` | the key, nothing else |
| `/budd:do-it` Steps 1 to 8 | that session, with subagents | the worktree, the handover sections, the document, the sidecar, the evidence set, the built tree | a GREEN standards gate per repo and every Expected bullet with a status |
| `/budd:finalize-it` | the same session (+ browser when owed) | `<anchor>/.git/budd-finalize/record.json` | a clean tree that matches the record |
| `/budd:commit-it` | the same session + GitHub | a commit, a pushed branch, an open draft PR per repo | the sealed record, or it is refused |
| hand-back | the same session | the report, the links, the comment draft, the Reply line | CI, or the 20-minute cap from `pr.ci_wait_minutes` |
| the comment | the tracker, on `post` | the closing comment through `ticket.comment_skill` or `budd ticket comment` | the human's `post` |
| `/budd:learn-it` | its own crew session, started by Budd's daemon | a `learn/<name>` branch of the kit repo | the session digest |

The split is deliberate.
The session is where work is investigated and built.
Budd's office is where the human sees what is waiting and answers with a click.
GitHub holds the diff and the draft PR, the only place a person reads the code before it is ready.
The tracker is the ticket's own record and the last stop.

## The hard gates

**Nothing starts without the human's typed command.**
`/budd:do-it` carries `disable-model-invocation: true`: the model cannot start it through the Skill tool, a subagent never runs it, and a request relayed from another session is not the human typing it.
Typing it is the only go-ahead the lane has, and it covers the build, the seal and draft PRs; never a ready PR, a merge, a base-branch push, or a tracker write.

**Nothing is committed before a finalize record is sealed.**
`budd guard` refuses `git commit` and `gh pr create` in a product repo unless `budd finalize check` passes for the exact tree being shipped.
It clears in two ways only: seal a record, or the **human** types `I_CONSENT=finalize-override`.
Never write that token yourself.

A record also goes stale when the tree moves or when the base gains commits in the same files, so finalize runs against what you are actually shipping, not what you shipped an hour ago.

**Nothing is sealed before the coding standard passes**, when the project has one.
The seal floor has four slots: lint, unit, build-or-e2e, and `standards`, which is `budd standards gate`.
It is green only when the deterministic scan finds no violation in the added lines and the `coding-standards` agent's PASS verdict was written for the exact tree being sealed.
A project with no `standards.cards` has a three-slot floor, and the seal and the hand-back say "standards: not configured".

**A destructive item is cleared only by the human's typed consent.**
The finalize scan names each one with a stable key; the session puts it on a Reply line as `I_CONSENT=finalize-approve budd finalize approve --key <key> …`, Budd renders Approve, and `budd gate` checks the token came from the human before `budd finalize approve` records it.
The consent log and the handover are the audit trail.

## What each step hands the next

- `/budd:do-it` leaves the expectation document in the handover log and as the sidecar (`E1` to `En`), the `cause` and `surface` sections, the evidence set with BEFORE frames, and a built tree with a GREEN standards gate.
  `/budd:finalize-it` reads the sidecar to check that every Expected bullet is evidenced before it seals.
- `/budd:finalize-it` leaves gates with their real exit codes, the review findings and how each was resolved, the destructive scan and its consents, and any browser frames it kept.
  `/budd:commit-it` builds the PR body out of that record: Verification from the gates, Risk Assessment from the findings and destructive items, Problem and Solution from the document.
- `/budd:commit-it` leaves the PR links and closes the handover.
  The hand-back carries those links, the evidence, and the comment draft.

The chain is one-way.
When something after the document contradicts it, that is a finding reported by id (NOT MET), never a quiet rewording.

## Skipping the chain

A trivial change does not need any of it.
A rename, a typo, a copy change, a style tweak or a one-line config edit is plan mode and a build, and the human saying "no process" or "quick and dirty" is the same answer.
The finalize gate still applies the moment you want to commit in a product repo, and the destructive gate applies everywhere.

A project may add its own lanes beside this one (a proposal lane with a human approval, a quick lane with a size gate) as project-local skills; they share the same gates and the same `budd` subcommands, and they add themselves to the rethinker classes through `config/rethinker.json`.
