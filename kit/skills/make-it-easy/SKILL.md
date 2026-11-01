---
name: make-it-easy
description: Rewrite ONE document in place so it reads fast top to bottom - the important parts first, direct words instead of jargon and paraphrase, every project term explained on first use, and the noise removed. Takes a local file path, or nothing (then it rewrites the last answer in this session). Overwrites the source. Trigger when the human invokes /budd:make-it-easy, or says "make this easier to read", "make this doc easy", "this is too hard to scroll through", "rewrite this so a newcomer gets it", "clean up this doc for reading". Not for a ticket and not for explaining a passage in the terminal (that is /budd:insimpleterms).
---

# make-it-easy - rewrite a document so it reads on the first pass

You point this at a document.
It replaces the document with a version a reader can scroll top to bottom and understand without stopping.

Same format, same file, same headings where they were already right.
The important things come first.
Every sentence says one thing in plain words.
Every project-specific term is explained the first time it appears.
Repetition and filler are gone.

## How it differs from the skill next to it

| Skill | Input | Output |
| --- | --- | --- |
| `/budd:insimpleterms` | a passage or an answer | a fixed seven-part explanation, printed in the terminal |
| `/budd:make-it-easy` | a whole document | the same document, rewritten and written back |

If the human wants to understand a passage, that is `insimpleterms`.
If the human wants the document itself fixed, that is this skill.

## Also the house style the kit's skills write in

`/budd:do-it`, `/budd:finalize-it`, `/budd:commit-it` and `/budd:grill-me` write every document and every terminal reply in this style from the first line, and run the Step 5 contract check before they record or commit.
Each carries a section titled `Written in the make-it-easy style` naming its outputs and what stays fixed.
The rules live here and only here.
A skeleton another tool or person reads by name stays fixed: commit subject and type, PR body section names, the expectation document's three headings, a tracker comment's first line and tables.
The rules apply to the prose inside it.
The `Read this first` block and the glossary go on pages written as markdown and on the PR body above its Problem heading.
Commit messages, tracker comments and grill questions get inline glosses only.

### Where it is enforced, and where it still is not

A promise in a skill document holds only as well as the writer's memory of it.
So the mechanical part is code where a lane has a shape check:

| Output | Enforced by | Status |
| --- | --- | --- |
| the expectation document | `budd expect check` | HARD BLOCK: the document is not shown |
| everything else the skills write | this document | not enforced |

The check covers only what needs no judgement: no sentence over 30 words, no em dash, no filler from the Step 3 list.
Blockquotes and code fences are exempt, because a verbatim quote is not prose we write.
A code span counts as one word, so precise identifiers cost the writer nothing.

The rest of Step 5 is still yours: whether a gloss explains the term, whether the opening block earns its place, whether the order serves the reader.
A machine cannot judge those, and pretending otherwise would make the gate fire on good writing and pass on bad.

## The two inputs

1. **A local file path.**
   Markdown, plain text, or a file whose body is prose.
   The rewrite is written back to that path.
2. **Nothing after the command.**
   The source is my last substantive answer in this session.
   The rewrite is printed as a new message.
   Nothing on disk changes.

Anything else is refused with one line naming the right skill.
A ticket is not a document for this skill.
A file marked auto-generated, and any `CHANGELOG.md`, is never rewritten.
A project may add a page transport (a wiki page read and written through its own connector) in a project-local skill; the rules here still apply to the prose.

## Who is reading

Two readers, one document.
Both must be served, and the document is layered so that they are.

- **The insider** has seen this project before and wants the point in ten seconds.
  They get it from the opening block and from headings that state a claim.
- **The capable newcomer** can read code and run commands but has never seen this project.
  They get it from a one-clause gloss on the first use of every project term.

General engineering terms are never explained.
`git`, `nginx`, `JSON`, `curl`, `symlink` stand on their own.
Project terms always are, once.
`the proxy` becomes `the proxy (the layer between the browser and the API)` the first time, and just `the proxy` after that.

## Step 1 - read the whole source

Read it in full.
A truncated read is not the source.
For a long file, read it in offsets until the end is reached.

Then list, for yourself, in one line each:

- what the document is FOR (what a reader does after reading it),
- every fact, number, path, warning, constraint and rule in it,
- every project term it uses without explaining.

The second list is the contract.
Every item on it is in the output or you have not finished.

## Step 2 - verify the context you will add

This skill is allowed to add what the source assumed.
It is allowed to do that only from something you read.

- A term's meaning comes from the code, the linked file, the project's instructions, or a knowledge card the project keeps.
- A "why" behind a rule comes from git history, a memory, or the sentence next to it. Not from what sounds plausible.
- If you cannot verify it, write `unknown` where the reason would go. Never guess to make a sentence complete.

Read those sources now, before rewriting, so the glosses are ready when the terms appear.

## Step 3 - rewrite

### The opening block

Every document opens with a block titled **Read this first**.
Three to seven lines.
Each line is one thing the reader must know or do, in the order they need it.
If the document is instructions, these are the rules that bite hardest.
If it is a report, this is the verdict and what changes because of it.

Nothing in the block is new.
It is lifted from the body and placed where a scanning reader stops.

### Headings

A heading states a claim or names an action.
`## The proxy validates Origin, not the API` beats `## Proxy`.
`## Run the lint gate before pushing` beats `## Linting`.
Keep a heading that was already a claim.
Keep the heading ORDER when the source is a reference somebody links into by anchor.
Otherwise order sections by how soon the reader needs them.

### Sentences

- One sentence per line in Markdown.
- One idea per sentence, about twenty words, always with a verb.
- Active voice, present tense. Name who does what.
- Say the thing, do not paraphrase around it. "Do not commit `config.php`" beats "care should be taken with tracked configuration files".
- Pick the verb that points at the goal. "Check the VPN status first" beats "it may be useful to look at the VPN".
- Pick the adjective that carries a fact. "tracked file" beats "important file".
- Cut hedges that carry no information: "basically", "essentially", "it should be noted", "in order to", "leverage", "utilize".
- Plain dash `-`, never an em dash.
- Backtick every path, command, table, column, identifier and flag.
- Numbers stay exact. Never round.

### Terms

First use of a project term: the term, then a gloss of one clause, in parentheses or after a comma.
Never rename the real thing to make it sound easier.
`stored procedure` stays `stored procedure`.
After the first use, the bare term.

### Markers the source relies on

Keep `STRICTLY ENFORCED`, `NEVER`, `READ-ONLY`, `DO NOT` and any tag another tool or hook greps for.
Keep code blocks, tables and diagrams as they are unless a cell is prose that needs the same treatment.
Keep every link and every anchor target that the rest of the repo points at.

## Step 4 - remove the noise, and only the noise

Noise is what the reader loses nothing by not having.
Remove it.

- The same point made twice in different words. Keep the clearer one.
- Filler and throat-clearing.
- Hedges that carry no fact.
- History the reader does not act on ("this used to live in another store", when nothing of it remains). If a date or a commit is the only place that history is recorded, it is a fact and it stays.
- A sentence that exists to introduce the next sentence.

Never remove:

- a fact, a number, a path, a command, a table name, an identifier,
- a warning, a constraint, a rule, a "never",
- a gotcha, even one that reads like trivia. "the client does not parse `host:port`" is an hour saved.
- a sentence that is hard to read but carries any of the above. Rewrite it.

When you are not sure whether a line is noise, it is not noise.
The removal is permanent for every reader who opens the file next.

## Step 5 - check the contract

Before writing anything back, go down the list from Step 1.
Every fact, number, path, warning, constraint and rule is present in the rewrite.
Every project term has a gloss on first use.
The opening block exists and says nothing the body does not.
No sentence is over about twenty words.
No em dash.

If one item is missing, put it back.
Do not write yet.

## Step 6 - write it back

The source is overwritten.
That is the decision, and the undo is history, not a sibling copy.

- **Tracked file**: overwrite. `git diff` is the review and `git checkout -- <path>` is the undo. Say both in the report.
- **Untracked file**: copy it first to the session scratchpad as `<name>.before.<ext>`, then overwrite. Print the backup path in the report. Without the backup there is no undo, so the copy is not optional.
- **Last answer**: print the rewrite as a new message. Nothing is written anywhere.

Never stage, never commit, never push.
The human decides whether the rewrite becomes a commit.

## Step 7 - report

Short.
The reader has a document to read, not a report.

```
**In simple terms:** rewrote <path> in place; <N> lines to <M> lines.

**What changed**
- Opening block added with <k> lines.
- <n> sections reordered / renamed to claims.
- <n> terms glossed on first use: `a`, `b`, `c`.
- Removed as noise: <one line naming what kind>. Nothing factual was removed.
- Context added from: <the files you read to verify it>.

**Undo:** `git checkout -- <path>` / backup at <scratchpad path>

**Next action for you, human:** open <path> and read it top to bottom once; tell me any line that still makes you stop.
```

The line count is measured with `wc -l`, not estimated.
If something in the source could not be verified and is now marked `unknown`, list it under **What changed** so the human can fill it.

## What this skill does NOT do

- It does not shorten by removing the hard part. It breaks the hard part into short sentences.
- It does not soften a bad result. A failing test stays failing, a blocked step stays blocked.
- It does not add anything it did not read and verify.
- It does not touch a second document because it noticed one. Report it, do not rewrite it.
- It does not commit.
