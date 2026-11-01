# PR code-review checklist

The single source of truth for HOW a change is reviewed in `/budd:finalize-it` Phase 1 (`/code-review` plus this checklist, then a review decision).
A project whose CI runs an LLM reviewer should embed this same structure in that reviewer's prompt, so a finding CI would block on is found here first.

Review in the **Senior Developer Review Order** (the 5-minute rule): spend attention on high-impact findings FIRST, formatting and style LAST. Check, in this order:

1. Does it solve the business problem?
2. Can it break production?
3. Is the logic correct?
4. Are tests sufficient?
5. Is it secure?
6. Is it performant?
7. Is it maintainable?
8. Only then, formatting and style.

The 8 points above are the PRIORITY order to spend attention in; the 14 numbered categories below are a COVERAGE checklist, not the reporting order.
Walk the categories to make sure nothing is missed, but report findings in whatever order is clearest.
Report ONLY categories with a real finding; do not restate what the diff already makes obvious.

## The 14 categories

1. **Requirements**: the implementation solves the ticket; acceptance items (the Expected bullets, by id) fully covered; missing edge cases; business rules implemented correctly.
2. **Correctness (HIGHEST PRIORITY)**: logic correct; no obvious bugs; null and undefined handling; boundary conditions; error handling present; no hidden side effects.
3. **Architecture & Design**: fits the existing architecture; no duplicated logic; proper separation of concerns; correct abstraction level; easy to extend later.
   The definitions and the scans are in `references/software-principles/` (`solid.md`, `kiss.md`, `yagni.md`).
4. **Readability**: meaningful names; method names explain intent; single responsibility per unit; complex code explained where needed; understandable without excessive comments.
5. **Maintainability**: follows project conventions and the coding standard cards; no dead code; no commented-out code; no unnecessary complexity; DRY.
   DRY here means one home per fact, not no repeated lines: `references/software-principles/dry.md`.
6. **Security**: input validation present; authorization checked; authentication not bypassed; injection prevented; XSS protection; no secrets committed; sensitive data not exposed.
7. **Performance**: no unnecessary database queries; no N+1 queries; large loops optimized; expensive operations minimized; caching considered where it helps.
8. **Database**: a migration is safe and backward compatible; indexes where appropriate; no destructive migration without justification; nothing here is applied by an agent (`database.writes` is `never`), it is a file handed to the DBA.
9. **API**: response format unchanged unless intended; validation updated; error responses consistent; versioning respected.
10. **Frontend**: responsive layout; loading, empty and error states; accessibility considered; cross-browser issues. (Backend-only PRs: mark n/a.)
    When the diff touches the user-facing UI, a confirmed overflow, occlusion, clipping or unreachable control under content and viewport stress is a REQUEST CHANGES, not a polish COMMENT.
    Judge against the diff plus the browser evidence already captured (the evidence set's frames, the finalize browser pass); no new browser session here.
11. **Testing**: existing tests still pass; new tests added; happy path covered; failure cases covered; edge cases covered; on a bug, the failing-then-passing pair exists.
11b. **Surface coherence**: do the words match the thing? A control's caption, the page title, the route and the handler name must all describe the same object; a primary action captioned for a different entity than the page lists is a REQUEST CHANGES, not a nitpick, because the user reads the caption rather than the handler.
12. **Logging & Monitoring**: useful logs added; no leftover debug statements; exceptions logged properly.
13. **Documentation**: README updated if needed; API docs updated; comments explain WHY, not what.
14. **Git Hygiene**: PR title is clear; description explains the change; commits are meaningful; no unrelated changes; no generated files or never-commit paths accidentally committed.

## Project specifics

A project folds its own stack rules into the categories above through its coding standard cards (`standards.cards` in the profile), not by editing this card.
Typical examples: which HTTP client carries the auth header, which layer may hold SQL, which proxy validates the origin.

## Review decision (end every review with exactly one)

- **APPROVE**: no functional issues; only minor suggestions remain.
- **REQUEST CHANGES**: a bug, security issue, performance issue, missing tests, unmet requirement, or poor architecture.
- **COMMENT**: naming, readability, refactoring suggestions, or future improvements only.

In `/budd:finalize-it`, an unresolvable **Correctness** (cat 2) finding, or a REQUEST CHANGES decision that cannot be resolved in the session, is a HARD block; cleanup and COMMENT-level findings are recorded in the finalize record, not blocking.

## Confidence and traceability (deterministic-first)

Every finding carries a **confidence tag** and a **citation** (`references/deterministic-first.md`):

- **High**: verified by deterministic evidence: cite it (`file:line`, a lint rule or analyzer message, a failing test id, a gate record, a screenshot path).
- **Medium**: partial evidence; name what is missing to raise it.
- **Low**: needs more verification; say what would confirm it.

Reason on top of facts you gathered with git and the lint commands (changed files, related files, lint on the changed files, diffstat), not on what you recall about the tree.
Assert a fact only with a citation; distinguish fact from assumption; if the evidence to conclude is absent, say so rather than guess.
A finding with no evidence path is a *question*, not a REQUEST CHANGES.
