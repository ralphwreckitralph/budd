# Contributing

- Every change goes in its own branch and pull request.
- `cargo fmt --check`, `cargo clippy --workspace -D warnings` and `cargo test --workspace` must pass on macOS, Windows and Linux.
- Tests that need a Claude login run locally only: `BUDD_E2E_REAL=1 cargo test -p budd-e2e -- --ignored`. Attach the log to the release pull request.
- Never commit anything from a Claude config folder: no transcripts, no `.claude.json`, no credentials. Fixtures are sanitized by `cargo xtask record-fixtures`.
