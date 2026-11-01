# Contributing

- Every change goes in its own branch and pull request.
- There is no CI by choice. Before a merge, run locally: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `cargo check --workspace --target x86_64-pc-windows-msvc` (check only; Windows runs happen on a Windows machine).
- Tests that need a Claude login run locally only: `BUDD_E2E_REAL=1 cargo test -p budd-e2e -- --ignored`. Attach the log to the release pull request.
- Never commit anything from a Claude config folder: no transcripts, no `.claude.json`, no credentials. Fixtures are sanitized by `cargo xtask record-fixtures`.
