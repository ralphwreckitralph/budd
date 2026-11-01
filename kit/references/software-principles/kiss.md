# KISS

> Ship the smallest thing that still answers the question, and one obvious path instead of a configurable one.

KISS is about the SIZE of what you build.
YAGNI (`references/software-principles/yagni.md`) is about whether you build it at all.
The two overlap, and a plan that fails both usually fails YAGNI first.

## What counts as a moving part

Anything somebody has to maintain, document, test or explain after this ticket closes:

- a new setting, flag, option or environment variable
- a new column, table or stored procedure
- a new abstraction, base class, interface, wrapper or composable
- a new screen, tab, modal or control
- a new branch in an existing function

Each one is a cost paid forever by everyone who reads the code next.
KISS is not a preference for short files, it is a refusal to add a moving part with no case behind it.

## The rules, concretely

- If a checkbox answers the question, ship the checkbox. Do not build a three-way selector for a two-state fact.
- Before adding a control, check whether an existing screen already does that job. Two ways to do one thing is worse than one way, even when the second is prettier.
- Prefer one obvious path over a configurable one. Configuration is a feature with its own bugs, its own docs and its own tests.
- No magic side effects. A save does exactly what the screen says it does. If a record needs creating, a human creates it deliberately.
- Reuse the pattern the repo already has rather than introducing a parallel one. A second pattern doing the same job is a permanent tax.

## The plan-time question (writing the expectation document)

Count the moving parts the change adds, out loud, and for each one name the concrete case in THIS codebase that needs it.
"As long as it makes sense" is the bar.
If you cannot point at a real case, cut it before the document is written.

## The review-time scan (`/budd:finalize-it`)

- A parameter, flag or option added by the diff with exactly one call site and no ticket line asking for it.
- A new abstraction with exactly one implementation and one caller.
- A branch that no test reaches and no requirement describes.
- A setting written into config that nothing reads except its own default.
- A save path that also creates, updates or deletes something the screen never mentions. That is not complexity, it is a magic side effect, and it is a blocker.

## When NOT to apply it

Simplicity is not the same as leaving things out.
None of these may be simplified away, ever:

- loading, empty, error and success states on a surface a person uses
- input validation, authorization checks and bearer checks
- the DENY default on a new permission
- error handling that would otherwise print a warning into a JSON or XML body

Those are requirements, not complexity.
Cutting one of them is not a simpler solution, it is an incomplete one.

## Severity

- **CRITICAL-BLOCKER** for a magic side effect: a code path that writes something the user's action did not describe.
- **MEDIUM** when the extra surface is persisted or public: a new column, table, setting or API field that somebody now has to keep alive.
- **NON-BLOCKING** when the extra complexity is local to one function and costs only reading time.
