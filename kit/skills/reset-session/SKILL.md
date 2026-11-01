---
name: reset-session
description: Hand this session over to a fresh one - none of the old turns, all of the facts. Writes a one-shot handoff brief (in a project worktree - worktree, branch, ports, env, servers, dirty files; anywhere else - working directory and, per git repo the session worked in, branch, ahead/behind, dirty files, last commits; plus where the work stands and what is next), arms it for THIS terminal only through `budd reset arm`, and hands the human a single /clear. The fresh session loads the brief automatically through the SessionStart hook. It is also the approval step of the context budget guard - when tools are blocked at the hard stop, the human typing /budd:reset-session unlocks them for the handover only. Trigger when the human invokes /budd:reset-session, or says "reset the session", "hand over to a new session", "handover", "context budget", "fresh session but stay on this worktree", "clear the context but keep the worktree", "start over on this branch".
---

# reset-session - same work, empty head

A long session gets expensive and vague at the same time.
The fix is not to keep going, it is to throw the conversation away and keep the work.

This skill does exactly that and nothing else.
It writes down what a fresh session would need in order to carry on, arms it for the next `/clear` in this terminal, and tells the human to hit `/clear`.
The next session comes up empty except for that brief.

It does not commit, does not push, does not touch the database, and does not stop or start servers.
It changes no code, and it writes nothing into any repo tree: the brief lives in the anchor repo's git dir.

## When the context budget guard sent you here

`budd budget`, run by the hooks on every tool call and prompt, watches every main session.
At the warn line (`kit.contextBudget.warn` in Budd's settings, 130,000 tokens by default) it warns; at the hard stop (`kit.contextBudget.stop`, 150,000 by default) it blocks every tool.
The only way out it offers is this skill, or `/compact` if the human prefers that.

**The human typing `/budd:reset-session` is the approval.**
That typed command unlocks the tools for this one turn, and for the handover only.
Do the four steps below and nothing else: no "one last fix", no new reading beyond what the brief needs.
`budd reset arm` locks the tools again the moment the brief is stored, and so does any later prompt that is not this command.
If the human answers a question in between, ask them to put the answer on the command line instead, for example `/budd:reset-session proj-950`.

You cannot approve this yourself.
A Skill call you make is not the human's command, and at the hard stop the Skill tool is blocked anyway.

## Step 1 - gather the facts

Decide from THIS session which kind of handover it is.

**In a project worktree.**
The evidence has to be from this session: a worktree path you actually read or wrote, a server you started there, a ticket you have been building in it.
Do **not** pick the most recently touched worktree on the machine.
Many exist and most are stale; a worktree somebody else left dirty is not "the worktree we are in".
If the session touched **two** worktrees, ask the human which one to come back up in. One question, then continue.

```bash
budd reset facts <name>
```

`facts` refuses (exit 3) if the worktree does not exist or, when the profile names a `worktree.env_file`, has none.
That refusal is the check; do not work around it.
It prints worktree, branch, ticket, the env file's values that are not secrets, how many of the servers `worktree.status` lists are alive, and per repo in `repos[]` the branch, uncommitted files (never-commit paths flagged), ahead/behind `origin/<base>`, and the last three commits.

`budd reset candidates` lists every worktree with servers / dirty / recency.
Use it to resolve a half-remembered name, never to guess which worktree the session was in.

**Anywhere else** (the project root, another checkout, another repo):

```bash
budd reset session-facts <path> [<path>...]
```

Name every git repo this session read or changed, one path each; with no path it uses the repo of the working directory.
It prints the working directory and, per repo, the branch, ahead/behind its upstream, the uncommitted files, and the last three commits.
Name only repos the session really worked in; the command never guesses them.

Either way, the block goes into the brief verbatim.
Do not re-derive any of it by hand, and do not edit its numbers.

## Step 2 - write the brief

The command supplies the facts. You supply the part only this session knows.

Write the file to the scratchpad, title line first, then the facts block, then these four sections, in this order:

```markdown
# Resuming <worktree name, or the goal in a few words>

<the facts block, pasted verbatim>

## What we are doing
Two or three sentences. The ticket or the goal in plain words, and why.

## Where it stands
What is already done and verified, and how it was verified. What is written but unproven.
A test that did not run is written as "not run", never as "passing".

## What is next
The next concrete action, first. Then the rest, in order.

## Decisions and constraints already settled
Anything a fresh session would otherwise re-litigate or get wrong: an approach the human
rejected, a file that must not be committed, a port, a login, a data caveat.
```

The `# ` title line becomes the fresh session's title when the brief is not for a worktree.

Rules for the narrative half, no exceptions:

- Only what happened. No plans that were discussed and dropped, no summary of the conversation.
- Unknown stays "unknown". A fresh session acting on a guess written here is worse than one that asks.
- Keep exact identifiers verbatim: paths, ports, branch names, table names, ticket keys.
- Keep it short enough to read. It is a handoff, not a transcript. Aim for under 60 lines including the facts block.
- If the session produced an expectation document, a finalize record or a handover section, link the path (`budd handover show --repo <anchor>`) instead of re-typing its content.
- Never paste a password, a token or any other credential into it.

## Step 3 - arm it

```bash
budd reset arm <name> --brief /path/to/brief.md     # in a project worktree
budd reset arm --brief /path/to/brief.md            # anywhere else
```

That copies the brief into `<anchor git dir>/budd-reset/brief-<pid>.md` and writes `pending-<pid>.json` next to it, where `<pid>` is the Claude Code process of this terminal.
Nothing lands in a repo tree, so nothing can be committed by accident.

`arm` refuses when it cannot identify this terminal's process, rather than writing a marker any session's `/clear` could take.
Run it through this session's Bash tool, which always carries it.

`arm` then relocks the context budget guard for this session, so tools are blocked again until the `/clear`.

## Step 4 - hand over the one action, then stop

Tell the human, in this shape:

> Brief armed for `<name or title>`.
> **Type `/clear` now** - the fresh session comes up with the brief already loaded.

Then the Reply line, so Budd shows it as a button: ``Reply: `/clear` ``.

That `/clear` is the human's to type.
You cannot reset your own session, so never claim you did.
Stop there. Do not start any new work in the old session after arming.

## What happens after /clear

`budd hook SessionStart` with matcher `clear` (the kit's `hooks/hooks.template.json`) reads `pending-<pid>.json`, returns the brief as `additionalContext`, sets the session title, and consumes the marker.
`/clear` keeps the same Claude process, so only this terminal picks the brief up.
A `/clear` in another session on this project has a different process and never sees it.

So the fresh session starts with the brief in context and an empty conversation, and the next `/clear` starts genuinely clean.

The hook fails open by design: if the process cannot be identified, the marker is stale, or the JSON is unreadable, it exits silently and the session is just a normal empty one.

**If the brief did not load** (the human quit and relaunched instead of `/clear`, which makes a new process, or the hook is not registered), recover it by hand:

```bash
budd reset show          # lists every armed brief
budd reset show <pid>    # prints one
```

`budd reset disarm [<pid>]` discards a marker without loading it; the brief file is kept.

## Notes and gotchas

- **One shot, six hours.** The marker is consumed by the first `/clear` in the same terminal and ignored after `kit.resetBriefTtlSeconds` (default 21600). A brief older than that describes work the human has probably moved on from.
- **Per terminal, not per project.** Parallel sessions on this project each get their own marker, keyed by their own process.
- **Servers survive the reset.** `/clear` ends a conversation, not the detached processes of a worktree. The brief records how many were alive; the fresh session re-checks with the `worktree.status` command rather than trusting the number.
- **Nothing is committed.** The brief is machine-local state. Do not add it to a PR.
