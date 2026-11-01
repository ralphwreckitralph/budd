//! `budd fingerprint`: the one definition of "the fingerprint of a working tree", from the pipeline's
//! `lib/fingerprint.sh`. Every record that has to prove it still covers the tree in front of it (the
//! finalize seal, the coding-standards verdict) compares against this; two copies would drift, and a
//! drifted fingerprint fails closed on one side and open on the other.
//!
//! The fingerprint is the git tree oid of the working tree: tracked changes plus untracked files that are
//! not ignored, minus the build output below, computed in a throwaway index so the user's real index is
//! never touched. It is identical before and after `git commit`, which is what lets one record cover the
//! commit and the PR, and it changes the moment any source file changes, which is what makes a stale
//! record fail closed.

use super::git;
use std::ffi::OsStr;
use std::fmt;
use std::path::Path;

/// Build output a repo leaves untracked WITHOUT gitignoring it (a frontend's `build/` is megabytes of
/// staging output). It is not source, CI never reviews it, and the gates themselves create it, so
/// counting it would invalidate the record the moment it was earned. Dropped from the fingerprint,
/// deliberately and visibly.
///
/// These are NOT passed to `git add` as `:!` pathspecs: a negative pathspec still names a path, so git
/// errors with "The following paths are ignored" the moment one of them is gitignored. Plain `git add -A`
/// already skips ignored paths in silence; these are removed from the throwaway index afterwards instead,
/// and only when they are not tracked in HEAD.
pub const EXCLUDE: &[&str] = &[
    "build",
    "dist",
    "dist-ssr",
    "coverage",
    "node_modules",
    ".reports",
    "test-results",
    "playwright-report",
];

/// Why there is no fingerprint: the step of the computation that failed, with what git said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    /// The step: `read-tree`, `add`, `write-tree`, or `index`.
    pub step: &'static str,
    /// What git (or the file system) said.
    pub detail: String,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = self.detail.trim();
        if detail.is_empty() {
            write!(f, "git {} failed", self.step)
        } else {
            write!(f, "git {} failed: {detail}", self.step)
        }
    }
}

impl std::error::Error for Error {}

/// The fingerprint of the working tree at `repo`, as [`EXCLUDE`] and the module doc describe it.
pub fn fingerprint(repo: &Path) -> Result<String, Error> {
    let scratch = tempfile::Builder::new()
        .prefix("lfi-idx.")
        .tempdir()
        .map_err(|error| Error {
            step: "index",
            detail: error.to_string(),
        })?;
    // The file must not exist yet: git creates the index at the path it is given.
    let index = scratch.path().join("index");
    let env: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", index.as_os_str())];
    let read = git(repo, ["read-tree", "HEAD"], &env);
    if !read.ok {
        return Err(Error {
            step: "read-tree",
            detail: read.stderr,
        });
    }
    let add = git(repo, ["add", "-A", "--", "."], &env);
    if !add.ok {
        return Err(Error {
            step: "add",
            detail: add.stderr,
        });
    }
    // Drop the build output, but only where it is untracked: a repo that COMMITS its build output keeps
    // it in the fingerprint, because there it is content under review. Anything gitignored never
    // entered the index, so this is a no-op for it.
    for name in EXCLUDE {
        if git(repo, ["cat-file", "-e", &format!("HEAD:{name}")], &[]).ok {
            continue;
        }
        let _ = git(
            repo,
            ["rm", "--cached", "-r", "-f", "--quiet", "--ignore-unmatch", "--", name],
            &env,
        );
    }
    let written = git(repo, ["write-tree"], &env);
    match written.line() {
        Some(oid) if !oid.is_empty() => Ok(oid),
        _ => Err(Error {
            step: "write-tree",
            detail: written.stderr,
        }),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fresh repository with one commit, or `None` with the reason printed when git is not there.
    pub(crate) fn repo() -> Option<(tempfile::TempDir, PathBuf)> {
        let temp = tempfile::tempdir().unwrap();
        // Canonical, since git prints canonical paths and macOS puts the temp folder behind a symlink.
        let root = temp.path().canonicalize().unwrap().join("repo");
        std::fs::create_dir_all(&root).unwrap();
        let init = git(&root, ["init", "-q", "-b", "main"], &[]);
        if !init.ok {
            eprintln!("skipping: {}", init.stderr.trim());
            return None;
        }
        assert!(git(&root, ["config", "user.email", "t@example.test"], &[]).ok);
        assert!(git(&root, ["config", "user.name", "Test"], &[]).ok);
        assert!(git(&root, ["config", "commit.gpgsign", "false"], &[]).ok);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        assert!(git(&root, ["add", "-A"], &[]).ok);
        let commit = git(&root, ["commit", "-q", "-m", "first"], &[]);
        assert!(commit.ok, "{}", commit.stderr);
        Some((temp, root))
    }

    #[test]
    fn the_fingerprint_is_stable_and_follows_the_tree() {
        let Some((_temp, root)) = repo() else { return };
        let clean = fingerprint(&root).unwrap();
        assert_eq!(clean.len(), 40, "{clean}");
        assert_eq!(fingerprint(&root).unwrap(), clean, "stable across runs");
        assert_eq!(
            git(&root, ["write-tree"], &[]).line().as_deref(),
            Some(clean.as_str()),
            "a clean tree's fingerprint is HEAD's tree"
        );
        assert!(
            git(&root, ["status", "--porcelain"], &[]).stdout.is_empty(),
            "the real index was not touched"
        );

        // A tracked file changes: the fingerprint moves.
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        let edited = fingerprint(&root).unwrap();
        assert_ne!(edited, clean);
        // It is the same before and after the commit.
        assert!(git(&root, ["commit", "-q", "-a", "-m", "second"], &[]).ok);
        assert_eq!(fingerprint(&root).unwrap(), edited);

        // An untracked source file counts; untracked build output and ignored files do not.
        std::fs::create_dir_all(root.join("build")).unwrap();
        std::fs::write(root.join("build").join("out.js"), "x").unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::write(root.join("ignored").join("x"), "x").unwrap();
        assert_eq!(
            fingerprint(&root).unwrap(),
            edited,
            "build output and ignored files are not counted"
        );
        std::fs::write(root.join("new.txt"), "x").unwrap();
        let with_new = fingerprint(&root).unwrap();
        assert_ne!(with_new, edited);
        assert!(
            git(&root, ["status", "--porcelain"], &[]).stdout.contains("?? new.txt"),
            "the real index still has nothing staged"
        );

        // Build output a repo commits stays in the fingerprint.
        assert!(git(&root, ["add", "-f", "build"], &[]).ok);
        assert!(git(&root, ["commit", "-q", "-m", "ship the build"], &[]).ok);
        let shipped = fingerprint(&root).unwrap();
        std::fs::write(root.join("build").join("out.js"), "changed").unwrap();
        assert_ne!(fingerprint(&root).unwrap(), shipped);
    }

    #[test]
    fn a_folder_that_is_not_a_repository_has_no_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        let error = fingerprint(temp.path()).unwrap_err();
        assert_eq!(error.step, "read-tree");
        assert!(error.to_string().starts_with("git read-tree failed"));
    }
}
