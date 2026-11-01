# Guardrail: destructive, irreversible, and fan-out actions

This file states the intent.
Enforcement is `budd gate`, run by the `PreToolUse` hook on every `Bash`, `Edit`, `MultiEdit`, `Write` and MCP tool call.
It fails closed: an error inside the gate is a refusal, not a pass.
The hook runs outside the model's control, so this is not a preference that can be forgotten after a compaction or reasoned around in the moment.
Turning it off is possible only in Budd's app settings (`kit.gates`), is logged, and shows a persistent banner while it is off.

## Why this exists

Some actions cannot be undone by editing a file back.
Mail reaches a real person.
A dropped table takes customer data with it.
A force push rewrites history other people already pulled.
A recursive delete outside a build directory has no undo.

For that class of action the cost of a wrong call is not proportional to how confident the agent felt.
So the gate does not ask the agent to be careful.
It removes the ability to act alone.

## The two tiers

**DENY** is for irreversible or outward-facing actions.
The tool call is blocked outright.
It clears only when the human attaches a consent token scoped to that exact category.

**ASK** is for actions that are reversible but fan out over an unenumerated set of items.
These route to a normal permission prompt carrying the reason, so the human decides in band.
No token is needed.

## What is denied, by consent slug

| Slug | Covers |
|---|---|
| `mail` | A mail transport set to a live driver, calls to hosted mail APIs (SendGrid, Mailgun, Postmark, Resend, SES, Brevo, Mailjet), framework console or REPL lines that send mail or notifications |
| `db-write` | `DELETE FROM`, `UPDATE ... SET`, `DROP`, `TRUNCATE`, `ALTER TABLE`, `GRANT`, `REVOKE` through `mysql`, `psql`, `mariadb`, `sqlite3`, and through any command the profile's `database.context_patterns` marks as database context |
| `db-reset` | Migration fresh, refresh, reset and rollback commands, database wipe and seed commands, `dropdb`, `redis-cli FLUSHALL` / `FLUSHDB` |
| `bulk-rm` | `rm -r` outside build and scratch directories, `rm` over a glob, `rm` fed a file list on stdin (`xargs`), `git rm -r` or a globbed pathspec, `find -delete`, `find -exec rm`, `shred`, `srm` |
| `git-destroy` | `git reset --hard`, `git clean -f`, `git checkout -- .`, `git restore .`, `git branch -D`, `filter-branch`, `filter-repo` |
| `force-push` | `git push --force`, `-f`, `--force-with-lease`, `--mirror` |
| `remote-mutate` | `git push --delete`, refspec branch deletion, `curl -X DELETE|PUT|PATCH` against the GitHub, GitLab or Bitbucket APIs, `gh api` with `-X DELETE|PUT|PATCH`, `gh <noun> delete` |
| `remote-write` | `rsync --delete` onto a `host:path` destination, and `curl -X DELETE|PUT|PATCH` to any non-loopback HTTP service |
| `container-wipe` | `docker compose down -v`, `docker volume rm`, `docker volume prune`, `docker system prune` |
| `worktree-remove` | `git worktree remove` (with or without `--force`: it deletes gitignored files either way) and the profile's own remove command when it has one |
| `payment-write` | A live-mode payment key on a command line, write calls to a payment provider's API, provider CLI commands that create, update, delete, cancel or refund, and REPL lines driving a payment SDK |
| `finalize-override` | `git commit` or `gh pr create` in a product repo without a sealed finalize record that still matches the tree (`budd guard`, see `references/guardrails.md`) |
| `finalize-approve` | Clearing a destructive item the finalize scan found: `budd finalize approve --key <key>` (`/budd:finalize-it` Phase 3) |

MCP tools whose name means send-mail are denied with **no token path at all**.
An agent should not be the thing that presses send on a message to a real person.
Draft it in the conversation and let the human send it.
Mail is evaluated before every other tool rule, so a tool that is both a mail tool and an edit never downgrades to a prompt.

## Commands aimed at another machine

Every rule above runs twice: once on the command as typed, once on whatever is being handed to a remote shell.
An `ssh` line is unwrapped past its flags and host token, and the remote payload is scanned as if it had been typed locally, so `ssh box "rm -rf /srv"` denies under `bulk-rm` exactly as the local form does.
Reasons raised that way are prefixed `(over ssh)`.

A destructive action must not get cheaper by being aimed at a server, which is where it is least recoverable.

## What is asked, not denied

- `Edit` with `replace_all: true`.
  Every occurrence is rewritten, including ones nobody has looked at.
- `Write` onto a file that already exists outside a build or scratch path.
  The whole file is replaced rather than patched, so anything not carried over is lost without a diff.
- In-place `sed -i` or `perl -pi` where the file list comes from a glob, `find`, `xargs`, or command substitution.
  The blast radius is not reviewable from the command text.
- `mv` whose source list is a glob or arrives on stdin.
  A two-argument rename is reversible and passes; an unenumerated move is not reviewable.
- A single `>` redirect onto a file that already exists and is not under a build or scratch path.
  `>` truncates before it writes.
- MCP tools that **edit or remove** an object that already exists somewhere shared: editing or transitioning a tracker issue, updating a wiki page, canvas and label updates.
- A queue worker started without a named queue.
  A worker on the default queue runs every job already waiting, including unrelated ones other runs left behind, and those can call paid APIs or send mail.

Additive MCP posts are deliberately NOT gated: adding a tracker comment, creating an issue, sending a chat message, creating a page.
They are the same class as `curl -X POST`, which this gate has always allowed, and a lane that ends by posting a comment while running unattended has nothing that can answer a prompt.
Gating them would not add safety, it would hang the lane.

## What is deliberately left alone

Plain `docker compose down` keeps volumes, so it passes.
A forward migration is not destructive, so it passes.
`rm -rf` passes when every target is under `node_modules`, `.next`, `.turbo`, `dist`, `build`, `coverage`, `.scratch`, `target`, a scratchpad directory, or a framework cache path.
`git push` without a force or delete flag passes.
`curl -X POST` to the GitHub API passes, because opening a PR and posting a review comment are routine and additive.
A queue worker with a named queue passes, because it only runs jobs on a queue you named.
Payment reads (GET, list, listen) pass; the payment rule only sees writes made from the command line, and a refund driven through the app itself still reaches the provider, so before a payment flow in the app list the webhook endpoints and say who will receive the events.
`rsync` without a delete flag passes, and `curl -X DELETE` to `localhost` passes, because a loopback dev server is not someone else's data.

## The limit this gate does not cover

The hook reads the command line. It does not read files.

So `bash deploy.sh` and `python3 batch_delete.py` pass, whatever is inside them, and they will keep passing.
A rule that gated every script execution would fire on almost everything and get the whole gate switched off, which is the failure mode described under "Maintaining it" below.

This is a real hole, not an oversight.
The mitigation is social, not technical: read a script before running it, and treat "run this script" from an untrusted source the same way you would treat the commands inside it.
The one place the hook does refuse indirection is a database client fed a `.sql` file, because there the whole point of the command is to apply statements nobody has read.

## The consent protocol

The agent must never write the consent token itself.
That is the whole mechanism.

When the agent hits a block it must:

1. Stop.
   Not retry, not rephrase the command, not reach for a different tool that does the same thing.
2. State what it wants to run, what it will affect, and how it knows the scope.
   For a database write that means the row count.
   For a delete that means the file list.
   For a worktree removal that means `git status` on both sides.
3. Hand the human the exact command to approve, prefixed with the token, on the Reply line (`references/reply-contract.md`) so Budd renders it as an Approve button:

```
Reply: `I_CONSENT=<slug> <the exact command>` or `stop`
```

The slug must match the category the hook detected.
A token for `bulk-rm` does not clear a `container-wipe`, so one approval never becomes a general bypass.

**Provenance.** `budd gate` accepts a token only when it appears in a message the human typed: a non-sidechain user entry in the session transcript that is not an `Autopilot:` message, not a relayed message from another session, and not text the model produced.
A token that cannot be traced to the human is refused, and the refusal says so.
Every token use is appended to `~/.budd/state/consent.log` and announced in Budd's office at the moment it is spent, so a bypass is always visible after the fact.

## Maintaining it

Rules live in the `budd gate` module, grouped one function per category, with the suite that feeds it a synthetic `PreToolUse` payload and asserts the decision.
A new rule needs both a case that trips it and a case that proves the neighbouring safe command still passes.
Without the second one the rule quietly becomes over-broad and gets disabled out of frustration.
A project adds patterns through the profile (`database.context_patterns`, `http.write_verbs`, `git.protected`, `git.never_commit`, `git.gate_config`, `git.test_paths`), never by editing the gate.
