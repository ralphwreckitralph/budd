# SOLID

> One unit, one reason to change, depending on an interface rather than on a detail.

SOLID is about where code LIVES and what it is allowed to know about.
It is the principle that decides whether a change lands in one file or leaks into eight.
Read it with KISS (`references/software-principles/kiss.md`): SOLID says put the seam in the right place, KISS says do not add a seam nobody needs.

## The five, in the shape a layered codebase already has

Most backends with a request layer, a domain interface, a persistence implementation and a container binding are already SOLID, so the check is almost always "does this change stay inside the existing shape", not "should we introduce a pattern".
A project names its own layers and the files that exemplify them in its coding standard cards.

| Letter | The rule | What it looks like |
|---|---|---|
| **S** Single responsibility | A unit changes for one reason. | The request handler reads the request, logs, calls the repository, responds. The SQL lives in the persistence file. Neither knows the other's job. |
| **O** Open/closed | Add behaviour by adding a unit, not by editing a shared one. | A new report is a new domain interface, a new persistence file, a new handler, and one binding line. No existing file grows a branch. |
| **L** Liskov substitution | Any implementation must keep the contract every caller already relies on. | A handler reads `data`, `message` and `meta` off whatever implements the repository. A second implementation returning a bare list is a break, even though the language accepts it. |
| **I** Interface segregation | An interface declares what its callers use, not everything a module can do. | A repository interface declares exactly the methods its handlers call. |
| **D** Dependency inversion | The caller depends on the abstraction, the container supplies the detail. | A handler type-hints the interface, and the container binds it to the implementation. A handler that constructs the implementation itself pins the feature to one database forever. |

On the frontend the same seams exist under different names.
One module owns transport and auth for every call.
Composables or hooks own reusable behaviour.
A view owns rendering.
A view that builds its own HTTP call has taken on a second responsibility, and the auth interceptor is the thing it silently loses.

## The plan-time question (writing the expectation document)

Before the document is written, answer for each step:

1. Which existing unit owns this responsibility today?
2. If the answer is "none", does the change add ONE new unit in the existing shape, or does it thicken a unit that already has a job?
3. Does anything in the plan reach past an interface to a concrete detail: a view constructing its own HTTP call, a handler writing SQL, a repository formatting a response?

A step that thickens an existing unit is allowed, but it goes into the Potential risk line by name, because everything else consuming that unit now shares the change.

## The review-time scan (`/budd:finalize-it`)

Per changed file, deterministically:

- Does a request-layer file contain SQL, or a persistence file build a response body? That is an S break with a real consequence: the query can no longer be tested or reused.
- Did a domain interface gain a method that only one implementation and no caller uses? That is an I break.
- Does a changed view call the raw HTTP client instead of the shared instance? That is a D break AND an auth defect.
- If the change added an implementation of an existing interface, does every caller of that interface still get the shape it reads? Name the callers.

## When NOT to apply it

- Do not introduce an interface with one implementation and no second one in sight. That is the abstraction YAGNI (`references/software-principles/yagni.md`) exists to stop, and it costs a reader one extra hop forever.
- Do not split a 30-line handler into four classes because a diagram looks tidier. Single responsibility is about reasons to change, not line counts.
- Legacy layers a project marks as such in its cards are not held to this. They have their own shape, and reshaping them is a separate, deliberate ticket.

## Severity

- **CRITICAL-BLOCKER** when the violation already produces a defect: a bypassed auth interceptor, a substituted implementation whose shape breaks a caller.
- **MEDIUM** when it puts a second copy of a read path or a business rule into the tree, or changes a shared unit with its other consumers unverified.
- **NON-BLOCKING** when it is placement or naming only and nothing behaves differently.
