# The root cause of a bug, on the ticket

Set the root cause on a Bug ticket at the same moment the ticket is updated, when the tracker has a field for it.
A Jira project often carries a cascading Parent and Child root-cause field with an automation that nags when a Bug is closed with the Child empty; a GitHub project may use labels; a `none` tracker has nowhere to put it and the PR body's `## Root Cause` is the record.

Shared card. Callers:

- the project's comment skill (`ticket.comment_skill`), right after the closing comment posts: a dropdown to the human.
- `/budd:commit-it`, right after the PR opens: the same dropdown, unless it runs inside `/budd:do-it`, which leaves the question to the comment so the run is not interrupted.

The field ids, option ids and the write call belong to the project's comment skill, because they are the tracker's and change per project.
This card holds what does not change: when it applies, how to ask, and how to classify.

## When it applies

- ONLY when the ticket's type is a Bug. Tasks, Stories and Sub-tasks do not carry these fields, so SKIP them and note "n/a, not a Bug".
- **A parent with no child is INCOMPLETE, not "already set".**
  A cascading field with only the top level chosen is exactly the state the automation nags about.
  Treat it as unfinished: keep the parent and fill the child.
  Say which parent you are completing, and if the parent itself looks wrong for the fix, flag that in one line rather than silently re-classifying it.
- If BOTH levels are already populated, LEAVE THEM.
  Never overwrite a human's complete classification.
  Note "already set" and move on.
- Non-blocking: if the field is absent, the tracker is not Jira, or the connector is unavailable, record the intended `Parent -> Child` in the hand-back and move on.
  Never block on root cause.
- Read the fields BEFORE offering anything, so the dropdown states the truth.

## Two modes: ask the human, or decide alone

Same catalog, different consent. Pick the mode from whether a human is actually there to answer.

### Interactive (a human is in the session): ONE dropdown, recommended first

1. Derive the best-justified pair from the diff first, using the Classification table below.
   You propose, the human confirms; arriving with no recommendation wastes their turn.
2. Ask ONE question, `header: "Root cause"`, up to 4 options.
   Option 1 is the derived pair with `(Recommended)` in the label; options 2-4 are the genuinely plausible alternates (usually the runner-up child under the same parent, plus the runner-up parent).
   Label each option `"<Parent> > <Child>"`.
   Put the one-line evidence in `description`: which changed line or file makes that category true.
3. `multiSelect: false`.
   The human can always pick "Other" and type a pair; map their text to the catalog.
   If it maps to no category, re-ask with the 4 closest children under the parent they named.
   Never invent a category name.
4. When the fields already hold a complete pair, do NOT ask.
   When only the parent is set, ask with all 4 options taken from THAT parent's children.
5. Write it, read it back, and report the pair with a one-line reason.
   If the human declines or answers something outside the catalog twice, leave the fields alone and say plainly that the child is still empty so the automation will nag.

Shape of the call:

```
AskUserQuestion questions: [{
  header: "Root cause",
  question: "Which root cause should <TICKET> be closed under?",
  multiSelect: false,
  options: [
    { label: "Logic/ Coding Error > Incorrect logic (Recommended)",
      description: "<file>:<line> returned <field A> under a <field B> label" },
    { label: "Logic/ Coding Error > Missing validation", description: "..." },
    { label: "Requirements/ Specification Gap > Incomplete or missing requirements", description: "..." },
    { label: "Database/ Data Issue > Incorrect query", description: "..." }
  ]
}]
```

### Autonomous (no human in the session): decide and record

An unattended caller runs with nobody to ask, and `AskUserQuestion` is not usable there.
Pick the best-justified category yourself, set it, and record `Parent -> Child` plus a one-line reason AND a confidence (High / Medium / Low) cited to the fix diff.
A category not traceable to a changed line is Low; prefer the most defensible category over a precise-but-unbacked guess (`references/deterministic-first.md`).

## Classification (pick the best-justified category from the fix)

Choose from what actually CHANGED in the diff, not from how the ticket was worded.
A wrong field read under a correct-looking label is a coding error even when the ticket calls it a mismatch. Common mappings:

- Wrong computation, off-by-one, null handling, missing validation or error handling in the product code -> **Logic/ Coding Error**
- UI misalignment, rendering, layout, responsive, clipping, overflow -> **UI/ Compatibility Issue** (child usually "Incorrect UI rendering"; use "Incorrect UI behavior" for wrong interaction, not just visuals)
- Wrong or missing SQL, schema mismatch, malformed or unexpected data -> **Database/ Data Issue**
- Proxy, API contract, unexpected API response, API auth -> **Integration/ API Issue**
- Missing or not-yet-ported feature (a parity gap) -> **Requirements/ Specification Gap** (child "Incomplete or missing requirements")
- Missing env var, wrong config, infra, network -> **Configuration/ Environment Issue**
- Security (authz, sanitization, data exposure) -> **Security Issue**
- If genuinely unclear -> **Unknown/ Under Investigation** (do not invent a specific cause)

## The catalog

The category names below are the common Jira root-cause catalog; a project's comment skill maps them to its own field ids and may carry more or fewer.
Format: `Category` then its children.

- Requirements/ Specification Gap: Incomplete or missing requirements; Ambiguous requirements; Requirement change not communicated; Incorrect business logic; Other
- Logic/ Coding Error: Incorrect logic; Off-by-one error; Null / undefined handling; Missing validation; Missing error handling; Other
- Database/ Data Issue: Schema mismatch; Incorrect query; Data migration issue; Invalid / unexpected / malformed data; Other
- Integration/ API Issue: Incorrect API contract; API authentication issue; Unexpected API response; Other
- Third-Party/ Dependency Issue: Dependency version mismatch; Breaking change in external library; Third-party service outage; Other
- Configuration/ Environment Issue: Missing environment variable; Wrong environment configuration; Infrastructure issue; Network / timeout issue; Other
- Deployment/ Release Issue: Feature flag misconfiguration; Missing deployment step; Wrong build deployed; Rollback not performed correctly; Other
- Performance/ Concurrency Issue: Race condition; Deadlock; Timeout; Memory leak; Other
- Security Issue: Broken authentication / authorization; Improper input sanitization; Exposed sensitive data; Insecure dependency; Other
- UI/ Compatibility Issue: Incorrect UI behavior; Incorrect UI rendering; Browser / device compatibility issue; Accessibility issue; Other
- Missing Test Coverage: Missing / lacking unit testing; Regression not caught; Edge case not considered; Test case not written; Other
- Incorrect Testing: Wrong expected result in test case; Wrong test data used; Test performed on wrong build / environment; Other
- Duplicate/ Mislogged Issue: Defect ticket already exists; Other
- Unknown/ Under Investigation: Unknown / Under Investigation
- Other: Other

## The parent-only case, worked

A Bug closed with Parent "Requirements/ Specification Gap" and NO child makes the automation nag the closer.
The completing write keeps the parent and adds the child; in interactive mode the four options offered are that parent's children minus the weakest.
When the existing parent and the diff disagree (the fix was a wrong field read, which the table maps to **Logic/ Coding Error > Incorrect logic**), say so in one line and let the human's dropdown pick decide.
Do not silently re-classify a parent a person chose.
