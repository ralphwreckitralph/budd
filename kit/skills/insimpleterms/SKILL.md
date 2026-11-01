---
name: insimpleterms
description: Rewrite a dense, rambling, or jargon-heavy block of text into a short, plain-language explanation the reader understands on the first pass - main topic, broken-down ideas, simplified words, the logic that connects them, a mental model, and a one-line bottom line. Trigger when the human invokes /budd:insimpleterms, or says "in simple terms", "simplify this", "explain that plainly", "I don't understand that", "too much yapping", "ELI5 this", "what does this actually mean". Also the standing output format for every hand-back and explanatory response of the kit's skills.
---

# insimpleterms

Turn a wall of text into something a tired human understands on the first read.

The input is either a paragraph the human pasted, a document you just read, or, most often, an answer you were about to write yourself.
The output is always the same seven-step treatment below.

## Non-negotiable rules

1. **Never add information the source does not support.**
   If the source does not say it, it does not go in the output.
   No filling gaps with plausible-sounding detail.
2. **Never drop meaning to make it shorter.**
   Simplifying is not summarising away the hard parts.
   If something is genuinely complicated, say it in short sentences, do not delete it.
3. **Say each thing once.**
   No restating the same idea in the intro, the body, and the summary with different words.
4. **No academic voice.**
   Short sentences. Everyday words. Active voice.
   Cut "it should be noted that", "in order to", "leverage", "facilitate", "utilize".
5. **Keep the technical term when the term is the point.**
   Do not rename a real thing to make it sound easier.
   Keep `stored procedure`, `symlink`, `JWT`, then explain it in one plain clause right after.
6. **State uncertainty as uncertainty.**
   If the source is vague or you are not sure, the output says so plainly.
   Never smooth over a gap to make the explanation read cleaner.

## The seven steps

Run these in order on the source text.

1. **Identify the main topic.**
   One simple sentence: what is this mainly about?
2. **Break it down.**
   Split the source into its individual ideas.
   Turn every long or nested sentence into several short ones, one idea each.
3. **Simplify the words.**
   Swap complex words for simple ones.
   Keep the terms that carry real meaning (rule 5) and gloss them inline.
4. **Explain the logic.**
   Show how the ideas connect, naming the relationship:
   cause -> effect, problem -> solution, action -> result, before -> after, claim -> evidence.
5. **Strip the excess.**
   Remove repetition, filler, and anything that is not in the source.
6. **Build a mental model** (when it helps):
   `Who/What -> Does what -> Why -> Result`
7. **Land a final summary.**
   Two lines maximum.
   After reading it, the human should not need to go back to the original.

## Output format

Use exactly this shape.
Drop a section only when the source genuinely has nothing for it (for example, no mental model for a pure list of facts).
Never add sections.

```
**Ticket: PROJ-980**            <- only when the answer is about a ticket; omit the line otherwise

**In simple terms:** <one sentence - step 1>

**What it says**
- <idea 1, short sentence>
- <idea 2, short sentence>
- <idea 3, short sentence>

**Terms worth keeping**
- `term` - <plain-language gloss, one clause>

**How it connects**
- <A> -> <B>   (cause -> effect)
- <problem> -> <solution>

**Mental model**
<Who/What> -> <does what> -> <why> -> <result>

**Bottom line:** <one or two lines>

**Next action for you, human:** <the one thing they do next - see the rule below>
```

Formatting notes:
- Bullets, not paragraphs. One idea per bullet.
- No sentence longer than about 20 words.
- Plain dash `-`, never an em dash.
- Backtick every file path, command, table, column, and code identifier.
- Numbers stay exact. Never round a figure to make it read nicer.

### The ticket header

When the answer is about a ticket, it opens with that ticket, on its own line, before anything else:

```
**Ticket: PROJ-980**
```

- **The key is written exactly as the tools write it**, in the form the tracker uses: a Jira key `PROJ-980` (upper case, one hyphen, no spaces), a GitHub issue `#980` or `owner/repo#980`, a brief `BRIEF-2026-10-09-slug`. That is the form the tracker, the branch and the worktree all use, so it stays copy-pasteable and searchable. Never `Proj - 980` or `proj980`.
- **One ticket, one line.** A response that genuinely spans two tickets names both, comma separated, in the order the answer treats them: `**Ticket: PROJ-980, PROJ-981**`.
- **No ticket, no line.** A question about the stack, a skill, the tooling or anything else with no ticket behind it simply starts at `In simple terms`. Never write `Ticket: none` or invent a key to fill the slot, and never guess one: derive it from the branch, the worktree or what the human named.
- It is a label, not a sentence. No summary, no status, no link on that line.

It exists because these answers are read days later and out of order, often beside three other tickets' worth of work.
The first thing the reader needs is which ticket they are looking at.

## The closing action (STRICTLY ENFORCED)

Every response in this format ends with one line naming what the human does next.
The line is worthless, worse than worthless, if what it points at is not there.

**The action must ALREADY be doable at the moment you write it.**

- **Never name something that does not exist yet.** If the action is "open the review page", the server is running and the URL is in the line. Do the thing that makes it real FIRST, then write the sentence. Telling the human to open something that was one tool call away from existing costs them a reply asking where it is.
- **Never write "say the word and I'll do X".** If X is mine to do and they have already asked for the outcome, do X and report it. Offering to do what they asked is a wasted round trip.
- **The action must be THEIRS.** "Re-run the gates" is my job, not theirs. Theirs is something only they can do: approve, decide between options I am not allowed to choose, log in, run a write path, or look at a screen and confirm what they see.
- **Every URL, port, path and file in the line is live and verified as you write it.** Not one you used earlier in the session and assume is still up. Servers exit and ports get reused. Check, then cite.
- **One action, not a plan.** If several things are pending, name the one that unblocks the rest and say the others are queued behind it.
- If the real next step is still blocked on work of mine, say that plainly and give them the action once it is ready. Never invent a placeholder to satisfy the format.
- When their action is to answer with a word or two (logged in, merged, approve), the Reply line follows the action line and is the last line instead. Its shape is in `references/reply-contract.md`.

## What this skill does NOT do

- It does not shorten by deleting the difficult part.
- It does not soften a bad result into a neutral one. A failing test is still reported as failing.
- It does not turn a verified fact into a hedge, or a guess into a fact.
- It does not restructure the human's own text when they asked you to preserve wording.

## Invoked directly

When the human runs `/budd:insimpleterms` with text, or points at a file, a ticket, a PR body, or a previous answer of yours:

1. Read the source in full first. Do not treat a truncated read as the source.
2. Run the seven steps.
3. Output the format above and nothing else: no preamble, no "here is the simplified version".

If no source is given, ask one question: which text should be simplified.

## Used as the standing gate

Every kit skill hands back in this shape: findings, root causes, plans, test outcomes, review verdicts, "here is what I changed", "here is why it broke", answers to a question.

It does not apply to short mechanical turns: a one-line acknowledgement, a "done" after a trivial edit, a clarifying question, a status line while tools are still running, or content whose format is fixed by another skill (commit messages, PR bodies, tracker comments).
Those keep their own format.

The gate in one line: if the human has to read it twice, it did not pass.

Hard rules that survive the simplification, no exceptions:

- Never trade accuracy for readability. A failing test is reported as failing, in plain words.
- Never invent detail to fill a gap. Unknown stays "unknown".
- Never drop the hard part to make the answer shorter. Break it into short sentences instead.
- Keep exact figures, file paths, table names, and identifiers verbatim.
- End with what the human needs to do or check next, and that action is already doable when you write it.
