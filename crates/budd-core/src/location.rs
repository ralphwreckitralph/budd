//! Where Claude Code keeps a session's transcript, and the project slug rule (from AvatarCore/TranscriptLocation.swift).
//!
//! The transcript is `<config>/projects/<slug>/<session id>.jsonl`, where `<slug>` is a folder the session
//! ran in, every character but ASCII letters and digits turned into "-". `claude agents` reports the
//! folder a session is in NOW, and a ticket run moves (`EnterWorktree` into its worktree):
//! pending-rollback-label kept writing to the folder it started in, while a resumed session writes under
//! its new one. The session id never changes, so the transcript is the newest file named after it, in
//! whichever project folder that is.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The project folder name Claude Code derives from `cwd`: every character that is not an ASCII letter
/// or digit becomes "-". Claude Code does this per UTF-16 code unit, so a character outside the Basic
/// Multilingual Plane becomes two dashes; everything else, "é" included, becomes one.
pub fn project_slug(cwd: &str) -> String {
    cwd.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() {
                vec![c]
            } else {
                vec!['-'; c.len_utf16()]
            }
        })
        .collect()
}

/// The newest `<session id>.jsonl` among `candidates`, the files named after the session the caller found
/// in the project folders, each with its modification time; where `cwd` says when there is none yet, for a
/// fresh session whose first message creates it. `config_dir` is already expanded (no "~").
pub fn transcript_path(
    session_id: &str,
    cwd: &str,
    config_dir: &Path,
    candidates: &[(PathBuf, SystemTime)],
) -> PathBuf {
    let mut newest: Option<&(PathBuf, SystemTime)> = None;
    for candidate in candidates {
        // The first of equally new files wins, as Swift's `max(by:)` keeps the first maximum.
        if newest.is_none_or(|(_, at)| candidate.1 > *at) {
            newest = Some(candidate);
        }
    }
    match newest {
        Some((path, _)) => path.clone(),
        None => config_dir
            .join("projects")
            .join(project_slug(cwd))
            .join(format!("{session_id}.jsonl")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// The files of the Swift test, as the caller would find them.
    fn files() -> Vec<(PathBuf, SystemTime)> {
        vec![
            (PathBuf::from("/c/projects/-Users-me-gamecenter/moved.jsonl"), at(100)),
            (PathBuf::from("/c/projects/-Users-me-assistant/here.jsonl"), at(100)),
            (PathBuf::from("/c/projects/-Users-me-gamecenter/resumed.jsonl"), at(100)),
            (
                PathBuf::from("/c/projects/-Users-me-worktrees-gamecenter-learn/resumed.jsonl"),
                at(200),
            ),
        ]
    }

    fn path(id: &str, cwd: &str) -> PathBuf {
        let file = format!("{id}.jsonl");
        let candidates: Vec<_> = files()
            .into_iter()
            .filter(|(path, _)| path.file_name().is_some_and(|name| name == file.as_str()))
            .collect();
        transcript_path(id, cwd, Path::new("/c"), &candidates)
    }

    #[test]
    fn a_session_still_in_its_folder_is_where_its_folder_says() {
        assert_eq!(
            path("here", "/Users/me/assistant"),
            PathBuf::from("/c/projects/-Users-me-assistant/here.jsonl")
        );
    }

    #[test]
    fn a_session_that_moved_is_found_by_its_id() {
        // pending-rollback-label, 2026-10-09: started in gamecenter, then EnterWorktree into its frontend.
        assert_eq!(
            path("moved", "/Users/me/gamecenter/version2/worktrees/x/frontend"),
            PathBuf::from("/c/projects/-Users-me-gamecenter/moved.jsonl")
        );
    }

    #[test]
    fn two_files_for_one_session_mean_the_newest_is_live() {
        // learn-casbo-1055: resumed in its worktree, writing under that folder from then on.
        assert_eq!(
            path("resumed", "/Users/me/gamecenter"),
            PathBuf::from("/c/projects/-Users-me-worktrees-gamecenter-learn/resumed.jsonl")
        );
    }

    #[test]
    fn a_fresh_session_is_expected_where_its_folder_says() {
        assert_eq!(
            path("new", "/Users/me/gamecenter"),
            PathBuf::from("/c/projects/-Users-me-gamecenter/new.jsonl")
        );
    }

    #[test]
    fn the_folder_name_keeps_only_ascii_letters_and_digits() {
        assert_eq!(project_slug("/Users/me/web server/é.x"), "-Users-me-web-server---x");
    }

    #[test]
    fn a_windows_folder_slugs_the_same_way() {
        assert_eq!(project_slug(r"C:\Users\x\proj"), "C--Users-x-proj");
        assert_eq!(
            transcript_path("s1", r"C:\Users\x\proj", Path::new("/c"), &[]),
            Path::new("/c")
                .join("projects")
                .join("C--Users-x-proj")
                .join("s1.jsonl")
        );
    }

    #[test]
    fn the_first_of_equally_new_files_wins() {
        let candidates = vec![
            (PathBuf::from("/a/s.jsonl"), at(5)),
            (PathBuf::from("/b/s.jsonl"), at(5)),
        ];
        assert_eq!(
            transcript_path("s", "/x", Path::new("/c"), &candidates),
            PathBuf::from("/a/s.jsonl")
        );
    }
}
