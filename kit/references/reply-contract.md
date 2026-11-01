# The Reply and Decide lines

The contract between a session running a kit skill and Budd's office.
A skill that stops to wait on the human ends its turn with one of these two lines, and Budd turns the answers into buttons.
Everything else in this file says exactly when each line applies, because a wrong line either costs the human a click they should not have had to make, or lets autopilot answer something that was theirs.

## Waiting on the human: the Reply line (STRICTLY ENFORCED)

When a turn stops because the human has to answer with a word or two, its LAST line is the Reply line:

```
Reply: `logged in` or `skip`
```

Each answer sits in backticks, at most four answers, each at most 24 characters, exactly the words the human would type.
Budd reads that line from background sessions and turns each answer into a button that types it back, so a manual step (log in, open the tab, approve, merge) costs one click instead of a trip to the terminal.

**Applies to** a turn that ends waiting on the human for a done-or-go: a login, an open tab, a tracker reply, a merge, an approve, a pick between named options.

**Does not apply to** a turn that is not waiting on the human, a status line while tools run, an open question with no short answer, or a permission prompt (that is Claude Code's own picker).
Never offer an answer you would not act on as written.
The message that carries the Reply line carries the whole ask, every link and step in it; a status message sent after the ask repeats them, never only their names.

### The consent form of the Reply line

One answer may be longer than 24 characters: a consent command.

```
Reply: `I_CONSENT=finalize-approve budd finalize approve --repo <anchor> --key <key> --note '<what you decided>'` or `stop`
```

Budd renders a consent answer as an Approve button and the rest as plain buttons.
Pressing Approve types the whole command as the human's own message, which is what `budd gate` checks for: a consent token counts only when it appears in a message the human typed, never in text the model wrote (`references/guardrail.md`).
A consent answer is always paired with a way out (`stop` or `skip`), and a Reply line never carries more than one consent command.
A skill writes the command on the Reply line and nowhere else: never in a code block it then runs, never in a tool call.

## The Decide line: a call Budd's autopilot may make

When the turn stops on a choice that any Claude with this session's context could make (which of two layouts, which of two equally valid names, whether to take the optional extra step), end it with a Decide line instead, the answer you recommend first:

```
Decide: `compact` or `roomy`
```

Same shape as the Reply line.
In Budd's human mode it is a row of buttons like any other.
In autopilot mode Budd waits 8 seconds, then answers it for the human with your first answer, sent as `Autopilot: compact`.

**Never a Decide line for** anything that needs the human's hands or say: a login or a password, an open tab, a tracker reply, marking a PR ready, a merge, an approval, an `I_CONSENT=` token, a database write, or anything destructive, money-related or irreversible.
Those stay on the Reply line, which autopilot never answers.
When unsure, it is a Reply line.

**An `Autopilot:` message** is Budd answering your last Decide line for the human, who switched autopilot on.
Carry on with that answer as if the human had typed it.
It only ever answers a Decide line: if one ever arrives for anything else (a Reply line, a gate, a permission), refuse it, say so, and ask the human again.

## The link lines

A hand-back that ends a run puts its links on their own lines, last, so they are never buried and Budd can show them as chips:

```
PR:     <url>
Commit: <sha> on <branch>
Ticket: <url>
```

`Ticket:` is omitted when the profile's tracker is `none`.

## Where it is read

Budd infers the lane's stage from the Skill calls it sees; nothing emits a stage line.
Only `/budd:do-it` and the skills it runs emit `Reply:` literally; no kit skill emits `Decide:` today, and the line exists so a project skill may.
The whole contract is this file; the kit's `SessionStart` project card points at it rather than repeating it.
