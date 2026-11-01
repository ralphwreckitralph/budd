# The kit's local marketplace

Claude Code installs a plugin from a marketplace, and a marketplace can be a local folder.
This folder is that marketplace: one manifest, `.claude-plugin/marketplace.json`, listing one plugin, `budd`, at `./budd`.

## How `budd kit install` lays it out

```
~/.budd/pipeline/                         <- this folder, copied here: the marketplace root
  .claude-plugin/marketplace.json         <- lists the plugin at ./budd
  budd/                                   <- the kit (the repo's kit/ folder), a git repository
    .claude-plugin/plugin.json
    hooks/hooks.json                      <- rendered from hooks.template.json at install
    bin/budd                              <- the app's binary, copied in
    skills/ agents/ references/ config/ state/
```

The app bundles both folders.
`budd kit install` copies the marketplace manifest to `~/.budd/pipeline/.claude-plugin/`, writes the kit to `~/.budd/pipeline/budd/` as a git repository (branches `shipped` and `mine`), then runs, against Budd's own `CLAUDE_CONFIG_DIR`:

```
claude plugin marketplace add ~/.budd/pipeline
claude plugin install budd@budd-kit --scope user
```

Claude Code loads the plugin in place from `~/.budd/pipeline/budd/`: `CLAUDE_PLUGIN_ROOT` is that folder, nothing is copied, and an edit takes effect at the next session start.
`budd kit update` changes only the `budd/` repository; the marketplace manifest changes only when the plugin's version does.

## Why a marketplace at all

A plugin folder passed with `--plugin-dir` loads for one process.
A plugin installed from a marketplace is registered in the config folder and loads for every session Budd starts, including background crew sessions and the hooks they run.
The marketplace is the registration, nothing more.
