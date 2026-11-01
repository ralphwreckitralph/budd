# DRY

> One fact, one home. Duplicate lines are cheap, duplicate KNOWLEDGE is what costs.

DRY is not "no two blocks of code may look alike".
It is that every business fact, figure and rule has exactly one place it is decided.
Two identical five-line blocks that change for different reasons are fine and should stay apart.
One figure computed in two places is a defect waiting for the next requirement change, because somebody will update one of them.

## Why this one is first among equals

The expensive version of a DRY break is a figure with two readers.
A report renders through the API read path, and an export renders the SAME figure a second time through a different file.
When only one of the two is changed, the table and the file disagree and both look correct on their own.
That is why `/budd:finalize-it` Phase 5 asks for a `crossView` comparison at all.

A project lists its own known duplicates in its coding standard cards, so a NEW copy is a finding rather than a style note; fixing the old ones is never this ticket's job.

## The plan-time question (writing the expectation document)

1. Does any figure, rule or validation in this change already have a home? Name the file.
2. Does this change create a SECOND reader of a figure it touches: an export, a companion app, a second report, a legacy version?
3. If the change copies an existing component or helper to modify it, say so in an Expected bullet, and say why extending the original is worse.

A deliberate copy is a legitimate answer.
An unnoticed copy is not, and the difference has to be visible in the document, not discovered in review.

## The review-time scan (`/budd:finalize-it`)

- Grep the repo for each new function, helper or component name added by the diff. A near-identical sibling is a finding, with both paths cited.
- For every figure the diff touches, enumerate its readers: the API read path, exports, any other report reading the same source. If more than one exists and only one changed, that is the finding.
- A constant or code string the diff introduces (a status, a category, a code) must be declared once and imported, never re-typed. A singular versus plural spelling of the same code is what re-typing costs.
- Copy-paste with one value edited is the highest-yield thing to look for in a diff, and it is visible in the diff alone.

## When NOT to apply it

- Do not merge two things that merely look alike. If they would change for different reasons, joining them creates a shared unit with two masters, which is worse than the copy.
- Do not refactor an existing duplicate that this ticket did not touch. Report it as a finding and leave it.
- Tests repeat themselves on purpose. A test that shares a helper with the code it tests can pass because both are wrong.

## Severity

- **CRITICAL-BLOCKER** when the duplicate already disagrees: two readers of one figure producing different values, two paths where only one enforces a permission or a bearer check.
- **MEDIUM** when the change adds a second home for a figure or a business rule, even if both are correct today.
- **NON-BLOCKING** when the repetition is local and mechanical, and no fact is stated twice.
