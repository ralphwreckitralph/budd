# Deterministic-first engineering

The governing discipline for every phase. One line:

> **Use code for certainty. Use AI for ambiguity.**

A deterministic step produces the same output for the same input (a git diff, a lint run, a field read, a curl).
It is predictable, testable, auditable, reproducible.
An LLM is probabilistic and can hallucinate.
So push every certain sub-step into code, and reserve the model for the parts that genuinely require reasoning, interpretation, or generation, feeding it a deterministic bundle and validating its factual output against deterministic sources.

The LLM is one component inside a mostly-deterministic pipeline, not the pipeline.

## Workflow order (every phase)

1. **Gather** deterministic evidence: the diff, the changed files, the lint output, the test exit codes, the ticket fields, the running page's text.
2. **Validate** the evidence (is it complete for this decision?).
3. **Apply** business and coding rules (the profile, the checklist, the coding standard cards).
4. **Reason** with AI, only now, on top of 1-3.
5. **Recommend / produce**, every factual claim traceable to an evidence item.
6. **Never guess.** If evidence is missing, state exactly what is needed (a `NEEDS_HUMAN` note), do not invent it.

## The code / AI boundary, per step of the lane

| Step | Deterministic (code owns it) | AI (genuine ambiguity only) |
|---|---|---|
| Read the ticket | fetch (`budd ticket read`), classify from the tracker's type field, attachments read verbatim | extract and number the messy reported items |
| Worktree | the profile's `worktree.create` and `worktree.up` commands | none |
| Find the code | `git log --since` and `git log -S` for an existing fix, the importers of a changed file | which code path the ticket's words name, the cause line |
| The document | `budd expect check` (shape), `budd expect sidecar` (ids) | the bullets themselves |
| Evidence | `budd evidence` manifest, headless shots from a recipe | which cells prove a bullet |
| Build | the team's owned paths (`budd team check`), the suites' exit codes | code generation, test authoring |
| Coding standard | `budd standards scan` (the `auto` rules) | the `agent` rules, graded by the `coding-standards` agent, recorded by `budd standards verdict` |
| Review | changed-file lint, diff, the principle cards' scans | `/code-review` reasoning, UI judgement |
| Gates | `budd finalize gate`: lint, unit, build, standards, extra gates, on real exit codes | none |
| Destructive scan | `budd scan destructive` over added lines against the merge base | the blast radius measured by a read-only count, the recommendation |
| Browser pass | `budd browser scope` decides whether one is owed; `budd browser verify` redoes the arithmetic and checks provenance against the captured page text | reading the page as a reviewer |
| Seal | `budd finalize seal`: every gate green, no flaky gate, every item consented with a rollback, every acceptance id evidenced, the tree fingerprint | none |
| Commit and PR | `git diff --stat`, the record's gates into the Verification table, `gh pr create --draft` | the PR body prose around injected facts |
| Hand-back | `budd evidence check`, `gh pr checks --watch` | the report prose |

The seal is the reference model: a fully deterministic gate with no AI.
The rest of the lane moves toward it.

## Facts and confidence

Every AI conclusion (a cause, a review finding, a root cause, any claim in a comment) carries a confidence tag and a citation:

- **High**: verified by deterministic evidence (cite it: `file:line`, a lint rule, a test id, a gate record, a screenshot path). Deterministic facts are High by definition.
- **Medium**: supported by partial evidence (name what is missing).
- **Low**: needs more verification (say what would raise it).

Rules: assert a fact only with a citation; distinguish fact from assumption explicitly; when the evidence to conclude is absent, say so and stop rather than guess.
Deterministic tooling is always preferred over an AI judgement that a script could have made reliably.
A `budd` subcommand that exits 2 has proved nothing, and "nothing was proved" is never passed forward as "clean".
