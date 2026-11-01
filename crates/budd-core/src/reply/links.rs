//! Something a session pointed the human at in its replies: a pull request, a ticket, a wiki page,
//! a local server, or a video it recorded. The crew card offers each as a button, so a link said a
//! few replies back is still one click away.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

static URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>()\[\]`"'*|]+"#).unwrap());
static VIDEO_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:~/|/)?[A-Za-z0-9_./~-]+\.(?:mp4|mov)\b").unwrap());
static PULL_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"github\.com/[^/]+/([^/]+)/pull/(\d+)").unwrap());
static ISSUE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"atlassian\.net/browse/([A-Z][A-Z0-9]+-\d+)").unwrap());
static LOCAL_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^https?://(?:localhost|127\.0\.0\.1)(?::(\d+))?").unwrap());
static REPOSITORY_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"github\.com/[^/]+/[^/]+").unwrap());

/// What kind of thing a link points at. The order is the order the card shows them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// A GitHub pull request.
    PullRequest,
    /// A Jira ticket.
    Issue,
    /// A Confluence page.
    Page,
    /// A server on this machine.
    Local,
    /// A video the session recorded.
    Video,
    /// Not found in replies: the ticket's evidence set, which the card adds itself.
    Evidence,
}

/// One thing a session pointed at, as a button: its kind, label and where it goes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplyLink {
    /// What it points at.
    pub kind: LinkKind,
    /// The button's label, such as "PR #256 FE" or "localhost:20300".
    pub title: String,
    /// A URL, or for a video the path as written: absolute, home-relative ("~/"), or relative to the
    /// session's folder.
    pub target: String,
}

impl ReplyLink {
    /// A link of `kind` with its label and target.
    pub fn new(kind: LinkKind, title: impl Into<String>, target: impl Into<String>) -> Self {
        ReplyLink {
            kind,
            title: title.into(),
            target: target.into(),
        }
    }

    /// A video's file, resolved against `folder`, the folder the session runs in, with "~" standing
    /// for `home`. `None` for any other kind of link.
    pub fn file(&self, folder: &str, home: &str) -> Option<PathBuf> {
        if self.kind != LinkKind::Video {
            return None;
        }
        let expanded = expand_tilde(&self.target, home);
        let path = if expanded.starts_with('/') {
            PathBuf::from(expanded)
        } else {
            Path::new(folder).join(expanded)
        };
        Some(normalized(&path))
    }

    /// How many of each kind are kept: a change often has a pull request in each of two repositories,
    /// but only the newest ticket, page, server and video are worth a button.
    fn limit(kind: LinkKind) -> usize {
        if kind == LinkKind::PullRequest { 2 } else { 1 }
    }

    /// The links in `texts`, oldest text first: the newest of each (a pull request per repository),
    /// at most `limit` of each kind, in a fixed order of kinds.
    pub fn collect<T: AsRef<str>>(texts: impl IntoIterator<Item = T>) -> Vec<ReplyLink> {
        let mut newest: HashMap<String, (ReplyLink, usize)> = HashMap::new();
        let mut order = 0;
        for text in texts {
            for link in Self::find(text.as_ref()) {
                order += 1;
                newest.insert(link.key(), (link, order));
            }
        }
        let mut entries: Vec<(ReplyLink, usize)> = newest.into_values().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        let mut counts: HashMap<LinkKind, usize> = HashMap::new();
        entries.retain(|(link, _)| {
            let count = counts.entry(link.kind).or_insert(0);
            *count += 1;
            *count <= Self::limit(link.kind)
        });
        entries.sort_by(|a, b| a.0.kind.cmp(&b.0.kind).then(b.1.cmp(&a.1)));
        entries.into_iter().map(|(link, _)| link).collect()
    }

    /// What two mentions of the same thing share, so only the newest is kept.
    fn key(&self) -> String {
        match self.kind {
            LinkKind::PullRequest => {
                let repository = REPOSITORY_PATTERN
                    .find(&self.target)
                    .map_or(self.target.as_str(), |found| found.as_str());
                format!("pr:{repository}")
            }
            LinkKind::Issue => format!("issue:{}", self.title),
            kind => format!("{}", kind as usize),
        }
    }

    /// Every link in one text that is worth a button, in the order written.
    pub fn find(text: &str) -> Vec<ReplyLink> {
        let mut found: Vec<(usize, ReplyLink)> = Vec::new();
        for range in Self::addresses(text) {
            if let Some(link) = Self::classify(&text[range.clone()]) {
                found.push((range.start, link));
            }
        }
        for video in VIDEO_PATTERN.find_iter(text) {
            if !is_inside_url(video.range(), text) {
                found.push((video.start(), ReplyLink::new(LinkKind::Video, "Video", video.as_str())));
            }
        }
        found.sort_by_key(|(at, _)| *at);
        found.into_iter().map(|(_, link)| link).collect()
    }

    /// Where each web address in `text` is, as byte ranges. A sentence's own punctuation after one,
    /// or Markdown's bold around it, is not part of it.
    pub fn addresses(text: &str) -> Vec<Range<usize>> {
        URL_PATTERN
            .find_iter(text)
            .map(|found| {
                let mut end = found.end();
                while end > found.start() && b".,;:!?".contains(&text.as_bytes()[end - 1]) {
                    end -= 1;
                }
                found.start()..end
            })
            .collect()
    }

    fn classify(url: &str) -> Option<ReplyLink> {
        if let Some(pull) = PULL_PATTERN.captures(url) {
            let title = format!("PR #{}{}", &pull[2], Self::side(&pull[1]));
            return Some(ReplyLink::new(LinkKind::PullRequest, title, url));
        }
        if let Some(issue) = ISSUE_PATTERN.captures(url) {
            return Some(ReplyLink::new(LinkKind::Issue, &issue[1], url));
        }
        if url.contains("atlassian.net/wiki/") {
            return Some(ReplyLink::new(LinkKind::Page, "Confluence", url));
        }
        if let Some(local) = LOCAL_PATTERN.captures(url) {
            let port = local
                .get(1)
                .map(|port| format!(":{}", port.as_str()))
                .unwrap_or_default();
            return Some(ReplyLink::new(LinkKind::Local, format!("localhost{port}"), url));
        }
        None
    }

    /// Which side of the product a repository holds, for telling two pull requests of one change apart:
    /// " FE" or " BE" when its name says, else the last word of its name with a space in front.
    fn side(repository: &str) -> String {
        let name = repository.to_lowercase();
        if name.contains("frontend") {
            return " FE".into();
        }
        if name.contains("backend") || name.ends_with("-api") {
            return " BE".into();
        }
        name.split('-')
            .rfind(|word| !word.is_empty())
            .map(|word| format!(" {word}"))
            .unwrap_or_default()
    }
}

fn is_inside_url(range: Range<usize>, text: &str) -> bool {
    URL_PATTERN
        .find_iter(text)
        .any(|url| url.start() < range.end && range.start < url.end())
}

/// `path` with a leading "~" replaced by `home`.
fn expand_tilde(path: &str, home: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_owned(),
    }
}

/// `path` with "." and ".." components resolved lexically, as Foundation's `standardizedFileURL` does.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::RootDir) | None) {
                    out.pop();
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_what_the_sessions_actually_print() {
        let text = "PR https://github.com/ralphwreckitralph/gamecenter-workspace/pull/53, ticket\n\
            https://gdptr.atlassian.net/browse/CASBO-1045?focusedCommentId=74631 and the proposal\n\
            https://gdptr.atlassian.net/wiki/spaces/CASBO/pages/1387462665. Test it at **http://localhost:20200/transactionquery/pendingtransaction**\n\
            and watch version2/worktrees/casbo-1056/video-evidence/CASBO-1056-evidence.mp4 first.";
        let links = ReplyLink::find(text);
        assert_eq!(
            links.iter().map(|l| l.kind).collect::<Vec<_>>(),
            [
                LinkKind::PullRequest,
                LinkKind::Issue,
                LinkKind::Page,
                LinkKind::Local,
                LinkKind::Video
            ]
        );
        assert_eq!(
            links.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            [
                "PR #53 workspace",
                "CASBO-1045",
                "Confluence",
                "localhost:20200",
                "Video"
            ]
        );
        assert_eq!(
            links[2].target,
            "https://gdptr.atlassian.net/wiki/spaces/CASBO/pages/1387462665"
        );
        assert_eq!(
            links[3].target,
            "http://localhost:20200/transactionquery/pendingtransaction"
        );
        assert_eq!(
            links[4].target,
            "version2/worktrees/casbo-1056/video-evidence/CASBO-1056-evidence.mp4"
        );
    }

    #[test]
    fn keeps_the_newest_of_each_and_a_pull_request_per_repository() {
        let links = ReplyLink::collect([
            "Old server http://localhost:20000 and https://gdptr.atlassian.net/browse/CASBO-970",
            "FE https://github.com/DWAsia/v2-game-center-frontend-casino-admin/pull/250",
            "FE again https://github.com/DWAsia/v2-game-center-frontend-casino-admin/pull/256, BE https://github.com/DWAsia/v2-game-center-backend-casino-admin-api/pull/124",
            "Now http://localhost:20300 for https://gdptr.atlassian.net/browse/CASBO-1045",
        ]);
        assert_eq!(
            links.iter().map(|l| l.title.as_str()).collect::<Vec<_>>(),
            ["PR #124 BE", "PR #256 FE", "CASBO-1045", "localhost:20300"]
        );
    }

    #[test]
    fn a_video_resolves_against_the_sessions_folder() {
        let relative = ReplyLink::new(LinkKind::Video, "Video", "version2/worktrees/x/../x/e.mp4");
        assert_eq!(
            relative.file("/Users/me/gamecenter", "/Users/me"),
            Some(PathBuf::from("/Users/me/gamecenter/version2/worktrees/x/e.mp4"))
        );
        let absolute = ReplyLink::new(LinkKind::Video, "Video", "/tmp/e.mp4");
        assert_eq!(
            absolute.file("/Users/me", "/Users/me"),
            Some(PathBuf::from("/tmp/e.mp4"))
        );
        let home = ReplyLink::new(LinkKind::Video, "Video", "~/e.mp4");
        assert_eq!(home.file("/", "/Users/me"), Some(PathBuf::from("/Users/me/e.mp4")));
        assert_eq!(
            ReplyLink::new(LinkKind::Local, "localhost", "http://localhost").file("/", "/Users/me"),
            None
        );
    }

    #[test]
    fn ignores_other_links() {
        assert!(ReplyLink::find("See https://docs.swift.org/x and https://github.com/DWAsia/repo/issues/3").is_empty());
    }
}
