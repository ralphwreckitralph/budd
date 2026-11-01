# The project profile

One JSON file per project tells the kit what the project is: its tracker, its repos, how a worktree is made, what lint and test commands prove a change, which paths are critical, and what may never be committed.
The skills never name a project fact themselves; they say "the lint command the project card names" and the profile supplies it.

## Where it lives

- Budd's folder: `~/.budd/profiles/<name>.json`, keyed by the project folder it binds to.
  Budd writes it from the app and `budd profile init`; it never writes into a repo.
- A repo may carry `.claude/budd-project.json`, found by walking up from the working directory, deep-merged over the Budd profile, so a team can commit its own profile.
  A gitignored `.claude/budd-project.local.json` beside it is merged last for per-machine values (a port, a login env name).
- `budd profile show` prints the merged result, `budd profile validate` checks it against this schema, `budd profile get <path>` prints one field (`budd profile get verify.app.lint`), and `budd profile card` prints the project card the `SessionStart` hook injects (`references/project-card.md`).
- Subagents and hooks re-resolve from their own working directory every time; nothing trusts an environment variable for a project fact.

`budd profile init` detects a fresh repo's commands from its manifest (`package.json` scripts, `composer.json`, `Cargo.toml`, `pyproject.toml`) and writes the minimal profile below.

## Minimal profile, single repo

Ten lines is a whole profile.
Everything not named takes the default documented in the full example.

```json
{
  "name": "toy",
  "ticket": { "kind": "github" },
  "repos": [
    { "id": "app", "path": ".", "remote": "owner/toy", "base": "main", "anchor": true }
  ],
  "verify": {
    "app": { "lint": "npm run lint", "unit": "npx vitest run", "build": "npm run build" }
  }
}
```

## Full profile, every field explained

Comments are not valid JSON; this block documents the fields, and `kit/examples/profiles/` holds files that parse.

```jsonc
{
  // The profile's name, also the key Budd files it under. Letters, digits, dashes.
  "name": "my-project",

  // The project folder this profile binds to. Budd fills it when the profile is created from the app.
  // A repo-local .claude/budd-project.json omits it: its own location is the root.
  "root": "~/code/my-project",

  "ticket": {
    // jira: read through the Atlassian connector when the session has it, else through `budd ticket read`
    //       with the REST token Budd keeps in its keychain.
    // github: `gh issue view` through `budd ticket read`; keys look like `#123` or `OWNER/REPO#123`.
    // none: there is no tracker; the human types a brief and the run gets a key `BRIEF-<date>-<slug>`.
    "kind": "jira",
    // Ticket key prefixes Budd's Start Tickets box accepts for this project, upper case.
    "prefixes": ["PROJ"],
    // Jira only: the site host. Ignored for github and none.
    "site": "example.atlassian.net",
    // Optional: a project-local skill that posts the closing comment on the ticket and knows the
    // tracker's custom fields (root cause, assignee). Without it the kit drafts a comment and
    // posts it with `budd ticket comment` on `post`.
    "comment_skill": "/jira-comment"
  },

  // One entry per repository the lane may change. Exactly one is the anchor: the repo whose git dir
  // holds the finalize record, the handover log and the reset brief.
  "repos": [
    {
      "id": "fe",                        // short id the other sections refer to
      "path": "frontend",                // relative to root, or absolute
      "remote": "owner/frontend",        // what `gh pr create --repo` gets
      "base": "develop",                 // the branch PRs target and worktrees fork from
      "anchor": true
    },
    { "id": "be", "path": "backend", "remote": "owner/backend", "base": "develop", "anchor": false }
  ],

  // How a worktree for a ticket is made and run. {key} is the ticket key in lower case, {base} the
  // anchor's base, {worktree} the resolved worktree path. Every command runs from root.
  // `budd worktree create|up|level|paths` run these; a skill never spells the command itself.
  "worktree": {
    "create": "git worktree add ../worktrees/{key} -b {key} origin/{base}",
    "resume": "",                        // optional: command to reattach to an existing worktree
    "up": "",                            // optional: start the worktree's servers, detached
    "status": "",                        // optional: print whether those servers listen
    // Where each repo lives inside the worktree. Defaults to the worktree itself for a single repo.
    "repo_paths": { "fe": "{worktree}/frontend", "be": "{worktree}/backend" },
    // Optional: a file in the worktree holding its ports and env; `budd worktree paths` prints its values.
    "env_file": ".worktree.env",
    // Optional: the URL the running app answers on, with placeholders from env_file.
    "app_url": "http://localhost:{FE_PORT}"
  },

  // Per repo id: the commands the finalize gates run, with their real exit codes recorded.
  // lint, unit and build (or a Playwright e2e the human asked for) are the seal floor.
  // A missing command is "not configured", which `budd finalize seal` reports and refuses to seal under,
  // unless `standards` is the missing one (see below).
  "verify": {
    "fe": {
      "lint": "npm run lint",
      "unit": "npx vitest run",
      "build": "npm run build",
      // Optional: lint ONE file, run by the PostToolUse hook on every product file written. {file} is the path.
      "file_lint": "npx eslint {file}",
      // Gate labels that have ever disagreed with themselves; they run with --repeat 2 and a disagreement is FLAKY.
      "flaky": [],
      // Project gates beyond the floor, each a label and a command exiting 0 only when green.
      "extra_gates": [
        { "label": "key-contract", "command": "bash scripts/key-contract.sh --base {base}", "when": "always" }
      ],
      // Optional: where to find the unit suite's coverage report for `budd coverage diff`.
      "coverage": { "kind": "lcov", "command": "npx vitest run --coverage --coverage.reporter=lcov", "report": ".cov/lcov.info" }
    },
    "be": { "lint": "composer run lint", "unit": "vendor/bin/phpunit", "build": "" }
  },

  // The coding standard the coding-standards agent grades against. Without cards the standards gate is
  // "not configured": the seal floor shrinks to lint, unit and build, and the hand-back says so.
  "standards": {
    "cards": ".claude/pipeline/coding-standards",            // a folder of rule cards, rules with ids
    "rules": ".claude/pipeline/coding-standards/rules.tsv"   // the `auto` rules `budd standards scan` applies
  },

  "git": {
    // Branches `budd guard` refuses to push to directly. The anchor's base is always included.
    "protected": ["develop", "main", "master"],
    // Paths that may never be staged or committed: local config, env overrides, test harnesses.
    "never_commit": ["config.php", "**/*.local", "apps/main/.env.development.local"],
    // Verification config the guard refuses to let an agent stage: CI, lint, type and test configuration.
    "gate_config": [".github/workflows/**", "eslint.config.*", "phpunit.xml", "vitest.config.*"],
    // Test paths: a staged deletion under them is refused with no bypass; test-author owns them.
    "test_paths": ["unit-fe/tests/**", "unit-be/tests/**"]
  },

  "database": {
    // "never" is the only value the kit ships. A schema or data change is a file for the DBA.
    "writes": "never",
    // Optional: the read-only query command `budd db read "<SQL>"` wraps. Omitted means no database reads.
    "read": "php scripts/db-read.php",
    // Patterns that mark a command or file as database context, so a mutating statement in it is refused.
    "context_patterns": ["mysql", "mariadb", "psql", "\\.sql$"],
    // Where a `.sql` file for the DBA may be written. Anywhere else, authoring mutating SQL is refused.
    "authoring_allowed": ["backend/docs/db/**"]
  },

  // HTTP verbs the guard treats as writes when a command calls the project's API.
  "http": { "write_verbs": ["POST", "PUT", "PATCH", "DELETE"] },

  // What makes a change critical, so /budd:do-it runs the rethinker and finalize owes a browser pass.
  // Everything else is UI/UX only. When in doubt the skill calls it critical.
  "critical": {
    "paths": ["backend/**", "frontend/api/**", "**/config.php", "**/*.sql"],
    "topics": ["auth", "session", "permission", "money", "schema", "server-setting", "deploy", "dependency", "irreversible"]
  },

  // The build team /budd:do-it spawns. Each role owns globs (relative to the worktree) and nothing else.
  // Defaults when omitted: test-author owns git.test_paths, one builder per repo owns that repo minus the
  // test paths, verifier owns nothing. `budd team brief <role>` renders the brief, `budd team check` holds
  // `git status` against the owned paths.
  "team": {
    "roles": {
      "test-author": { "owns": ["frontend/unit-fe/tests/**", "backend/unit-be/tests/**"] },
      "fe-builder": { "owns": ["frontend/**"], "except": ["frontend/unit-fe/tests/**"], "frozen": ["frontend/config.php"] },
      "be-builder": { "owns": ["backend/**"], "except": ["backend/unit-be/tests/**"], "frozen": ["backend/app/settings.php", "backend/app/routes.php"] },
      "verifier": { "owns": [] }
    },
    // The file both builders share the API contract through, inside the handover directory.
    "contract": "contract.md"
  },

  // How a question reaches the human. "reply" is the only transport the kit ships: a Reply line Budd
  // renders as buttons. A project skill may add its own and name it here for its own lanes.
  "ask": { "transport": "reply" },

  "evidence": {
    // true only when the evidence-shots module is installed (Node and Playwright Chromium under ~/.budd/modules/).
    // The evidence manifest (`budd evidence init|screen|proof|check|show`) is always on.
    "shots": false,
    // Env variable names (never values) the headless shot reads its test login from.
    "login_env": { "user": "APP_TEST_USER", "password": "APP_TEST_PASSWORD" }
  },

  "pr": {
    // /budd:do-it always opens drafts. false lets /budd:commit-it typed by the human open a ready PR.
    "draft": true,
    // How long the hand-back waits on CI before reporting the checks still running.
    "ci_wait_minutes": 20
  }
}
```

## Which skill reads which field

| Skill | Fields |
| --- | --- |
| `/budd:do-it` | `ticket.kind`, `ticket.prefixes`, `ticket.comment_skill`, `repos[]`, `worktree.*`, `verify.*.lint`, `verify.*.unit`, `verify.*.flaky`, `standards.cards`, `git.test_paths`, `database.writes`, `database.read`, `database.authoring_allowed`, `critical.*`, `team.*`, `evidence.*`, `pr.*` |
| `/budd:finalize-it` | `repos[]`, `worktree.app_url`, `verify.*` (every key), `standards.*`, `git.never_commit`, `git.gate_config`, `git.test_paths`, `database.read`, `critical.paths`, `evidence.shots`, `ask.transport` |
| `/budd:commit-it` | `ticket.kind`, `ticket.comment_skill`, `repos[]`, `git.protected`, `git.never_commit`, `git.gate_config`, `git.test_paths`, `pr.draft` |
| `/budd:learn-it` | none from the project; it reads the kit repo and Budd's session digest |
| `/budd:reset-session` | `repos[]`, `worktree.env_file`, `worktree.status` |
| `/budd:grill-me` | `ask.transport` |
| `/budd:insimpleterms` | `ticket.prefixes` (the ticket header's form) |
| `/budd:make-it-easy` | none |
| `rethinker` agent | `critical.*` (through the CRITICAL line it is given) |
| `coding-standards` agent | `standards.cards`, `standards.rules` |
| `captain` agent | none |

## Rules

- A value the profile does not have is reported as "not configured", never guessed by a skill.
- `database.writes` is `never` and nothing in the kit reads it any other way; it is in the schema so the fact is written down where a project looks for it.
- The profile holds names of env variables, never secrets.
- Budd never edits a repo's `.claude/budd-project.json`; `budd profile init` prints it for the human to commit.
