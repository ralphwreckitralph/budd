# YAGNI

> Build what the ticket asks for. Nothing on speculation.

You Aren't Gonna Need It is the discipline of not building for a future nobody has asked for.
KISS (`references/software-principles/kiss.md`) asks how complicated the thing is.
YAGNI asks whether it should exist in this change at all.

The test is one sentence: **every Expected bullet, and every hunk of the diff, traces to a quoted ticket line, a reproduced defect, or a gate that demanded it.**
A step whose only justification is "we will need it later" is not built.
It is written down as a finding for the hand-back and left for the ticket that actually needs it.

## What speculation looks like

- A generic handler, wrapper or base class introduced for one caller, because a second one is expected.
- A parameter added to a repository method that nothing passes yet.
- A database column added "while we are in there", so a later ticket has somewhere to write. A column that already exists and is empty is a finding, not free space.
- An API field returned that no screen renders.
- A settings toggle for behaviour nobody has asked to toggle.
- Fixing an adjacent bug the ticket never mentioned, inside the same PR. That is a FINDING to report, not a silent extra fix.

The last one is the easiest to get wrong, because it feels like generosity.
It widens the diff, widens the review, and puts an unrelated regression risk into a PR somebody approved for a different reason.

## The plan-time question (writing the expectation document)

For every Expected bullet, name the ticket line or reproduced defect it closes.
A bullet with no such line is cut before the document is shown.

Two things that are NOT YAGNI violations and must not be cut:

- Tests. Coverage of the change is part of the change.
- A design that survives the next real case without a rewrite. Nullable and additive beats a forced backfill later, and picking the shape that does not need rewriting is not speculation, it is the same amount of work done once.

## The review-time scan (`/budd:finalize-it`)

Walk the diff against the Expected bullets and name the ones each hunk serves.
Findings:

- A changed or added file that no bullet, defect or gate explains.
- An added function, parameter, field or column with no caller, no reader and no test.
- Dead scaffolding: commented-out code, a stub for a future case, a TODO that describes work this ticket is not doing.
- Schema or API surface the ticket never asked for. That one does not get resolved in review: it goes to the human as a destructive item in Phase 3, because it outlives the ticket.

## When NOT to apply it

- Security is never YAGNI. A permission check, a DENY default, an input validation and a bearer check are part of "done" even when the ticket did not spell them out.
- Neither are the states a surface can reach: loading, empty, error, success.
- Neither is a rollback path for anything destructive. `/budd:finalize-it` refuses to seal without one, and writing it afterwards is always worse.

## Severity

- **CRITICAL-BLOCKER** when the speculative work is destructive or persisted in a way that is hard to walk back: an unrequested schema change, an unrequested write path, a new outward send.
- **MEDIUM** when it widens the public surface: an unrequested API field, setting or permission, or an out-of-scope fix riding along in the same PR.
- **NON-BLOCKING** when it is local and inert: dead code, an unused parameter, a stub nothing calls.
