---
name: grill-me
description: Stress-test a plan or a vague instruction by resolving every unresolved branch of the design, each question with a recommended answer. Explores the codebase, git history and prior PRs first, and only asks what needs human judgement (scope calls, product intent, tradeoffs). Asks in the terminal, one question per message, each ending on a Reply or Decide line so Budd renders the answers as buttons; a project may add its own transport (a page, a chat) through `ask.transport`. Trigger - /budd:grill-me, or "grill this plan".
---

# grill-me: plan stress-test protocol

You are grilling a plan, or a vague instruction, before code gets written.
The goal is to resolve every unresolved branch of the design with the least human effort.

**The questions go where the human can answer them.**
The kit's transport is the terminal plus Budd: each question is one message that ends on a Reply line (or a Decide line when any Claude could make the call), and Budd turns the answers into buttons on the session's card, so the human answers from wherever they are.
A project may name another transport in `ask.transport` (a wiki page the team can reply on, a chat thread); then its own skill carries the question there and this skill's rules still hold.

Say which transport you are using, in one line, before the first question.

## Written in the make-it-easy style

Every question this skill posts, every reply it writes, and every terminal reply is written in the `/budd:make-it-easy` style.
The rules are in `skills/make-it-easy/SKILL.md` and are not repeated here.
Short version: one idea per sentence, direct verbs, every project term glossed in one clause on first use, repetition and filler removed, never a fact, a number, a path, a warning or a rule.
Written that way from the first line, never as a second pass over a finished draft.
Terminal replies inside this skill keep the `/budd:insimpleterms` shape and take the sentence rules on top.

What that means here:

- **A question is one message with a fixed order**: the claim in bold, the recommendation, the reason, what it costs, the ask. That order IS the skeleton and stays.
- **Inline glosses only.** A question has no opening block and no glossary. The first project term in it gets one clause, then the bare term.
- **The reader may not have done the investigation.** Write the reason in what they can check, not in what you read: "the deposit path was fixed in PR #482" beats a file path.

## Rules

1. **One question per message.** Never dump a list into one message. Each question gets its own message and its own Reply line, so each answer arrives against the question that produced it and there is no guessing which reply answered what. A terminal message with four questions is the wall of text this skill exists to avoid.
2. **Independent questions do not wait on each other; dependent ones do.** Post each question whose answer does not change another question as soon as it is ready, and hold back only a question whose wording depends on an earlier answer, posting it once that answer lands. Number them (`1 of 4`) so the reader knows how long this is going to take, and fold all the answers of a batch into ONE revision of the plan.
3. **Explore before asking.** If reading the codebase, git history or a prior PR answers it, go find the answer and skip the question. The human gets only what needs their judgement: scope calls, product intent, tradeoffs, anything that costs money or is irreversible.
4. **Every question ships with a recommended answer and the reason.** State the recommendation first, then the evidence, then ask. "Recommended: keep it scoped. The deposit path was fixed in PR #482 and nothing in this ticket reaches it. Agree?"
5. **A question the reader cannot answer is not a question.** An unverified fact is a risk, not a decision. If you could not check whether a value is set on a server, say so in the plan's risk, labelled unknown; do not put it in front of somebody who has no way to answer it.
6. **Track every resolution into the plan, in ONE revision per batch.** An answer is not resolved until it is written into the plan the session holds. Wait until every question in the batch is answered, then write all the answers into one revision. Reply under each question as its answer arrives, saying which revision will carry it. A reply that is a question back does not hold the batch: answer it, and revise the rest.
7. **Know when to stop.** When no unresolved branches remain, say "All branches resolved" and show the updated plan, and stop. Never proceed to coding from inside this skill, and never write an approval sentence or a consent token in any form.

## The shape of a question

```
**Question 1 of 3 - the CSV computation footnotes.**

Recommended: add them. Three sibling reports already append a `Computation notes` block, and this
report's total is count x a fixed multiplier that is invisible in the file.

Against: it writes those multipliers down in a fourth place.

Reply: `add them` or `leave it out`
```

The last line is a Reply line when the human has to decide, and a Decide line when any Claude with the session's context could make the call and the human has autopilot on (`references/reply-contract.md`); when unsure, it is a Reply line.
Never a Decide line for scope, money, data, auth or anything irreversible.
An answer that is not one of the offered words is still an answer: carry it into the plan in the human's terms.

## What counts as an unresolved branch

- Scope edges: the same defect exists elsewhere, does this change include it?
- Ambiguous acceptance criteria: two readings of the ticket produce different code.
- Risky surfaces: money flows, migrations, auth, anything irreversible.
- Hidden coupling: the change touches code shared with another feature or a second reader.
- Missing constraints: no target given: a perf number, a browser set, a locale set.
- A principle knowingly broken where the alternative costs something a human should choose: extending a shared component versus copying it, adding a persisted column.

## Where it is not run

On the ticket chain (`/budd:do-it` through `/budd:commit-it`) nothing goes to this skill: the rethinker's points are folded into the expectation document before it is shown, or fixed and reported after the build (`config/rethinker.json`).
Only the human starts it, on a plan they are discussing in the session, or the rethinker gate names it after a skill outside that chain.

## Recording the resolution

An answer changes the document, or it did not count.

1. Once every question in the batch is answered (rule 6), write each answer into the plan in the human's terms, as a decided row that names who decided it, and bump the plan's revision once.
2. Show the revised plan, with one line per answer saying which question it closes.
3. If a project transport carried the questions elsewhere, its skill posts the same answer under each question there, so the record stays attached to the question.
