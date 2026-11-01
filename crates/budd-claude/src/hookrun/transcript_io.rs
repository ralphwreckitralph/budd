//! Transcripts on disk: a file as a `TranscriptSource`, a config folder as a `TranscriptStore`, finding a
//! session's transcript, and how full a session is (from session_size.py).

use budd_core::history::{Candidate, TranscriptChain, TranscriptSource, TranscriptStore};
use budd_core::location::transcript_path;
use budd_core::transcript::usage_tokens;
use serde_json::Value;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A transcript file, read by byte range each time it is asked, so it sees the file grow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsTranscriptSource(pub PathBuf);

impl FsTranscriptSource {
    /// The transcript at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }
}

impl TranscriptSource for FsTranscriptSource {
    fn len(&self) -> u64 {
        fs::metadata(&self.0).map(|m| m.len()).unwrap_or(0)
    }

    fn read(&self, range: Range<u64>) -> Vec<u8> {
        let Ok(mut file) = File::open(&self.0) else {
            return Vec::new();
        };
        if range.end <= range.start || file.seek(SeekFrom::Start(range.start)).is_err() {
            return Vec::new();
        }
        let mut bytes = Vec::new();
        let mut reader = file.take(range.end - range.start);
        if reader.read_to_end(&mut bytes).is_err() {
            return Vec::new();
        }
        bytes
    }

    fn identity(&self) -> u64 {
        fs::metadata(&self.0).map(|m| file_identity(&self.0, &m)).unwrap_or(0)
    }
}

/// What tells a file from one that replaced it at the same path: its inode on Unix; elsewhere a hash of
/// its path and creation time, since the stable standard library gives no file index on Windows.
fn file_identity(path: &Path, metadata: &fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = path;
        metadata.ino()
    }
    #[cfg(not(unix))]
    {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        if let Ok(created) = metadata.created() {
            created.duration_since(UNIX_EPOCH).unwrap_or_default().hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The transcripts under a Claude config folder: `<config>/projects/*/*.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsTranscriptStore {
    /// The config folder, the one above `projects`.
    pub config_dir: PathBuf,
}

impl FsTranscriptStore {
    /// The store for the Claude config folder `config_dir`.
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    /// Every transcript file under `projects`, in no particular order.
    pub fn transcript_files(&self) -> Vec<PathBuf> {
        transcript_files(&self.config_dir, None)
    }
}

impl TranscriptStore for FsTranscriptStore {
    fn open(&self, path: &str) -> Option<Box<dyn TranscriptSource + '_>> {
        let path = PathBuf::from(path);
        path.is_file()
            .then(|| Box::new(FsTranscriptSource(path)) as Box<dyn TranscriptSource + '_>)
    }

    fn candidates(&self) -> Vec<Candidate> {
        self.transcript_files()
            .into_iter()
            .filter_map(|path| {
                let metadata = fs::metadata(&path).ok()?;
                let created = metadata.created().or_else(|_| metadata.modified()).ok()?;
                let created_at = i64::try_from(created.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
                Some(Candidate {
                    path: path.to_string_lossy().into_owned(),
                    created_at,
                    head: head_of(&path, TranscriptChain::HEAD_BYTES),
                })
            })
            .collect()
    }
}

/// The first `bytes` of the file at `path`, or nothing when it cannot be read.
pub fn head_of(path: &Path, bytes: usize) -> Vec<u8> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let mut head = Vec::new();
    if file.take(bytes as u64).read_to_end(&mut head).is_err() {
        return Vec::new();
    }
    head
}

/// The `.jsonl` files under `<config>/projects/*`, all of them or only those named `<only>.jsonl`.
fn transcript_files(config_dir: &Path, only: Option<&str>) -> Vec<PathBuf> {
    let wanted = only.map(|name| format!("{name}.jsonl"));
    let Ok(projects) = fs::read_dir(config_dir.join("projects")) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let matches = match &wanted {
                Some(wanted) => name == wanted,
                None => name.ends_with(".jsonl"),
            };
            if matches && path.is_file() {
                found.push(path);
            }
        }
    }
    found
}

/// Where the session's transcript is: the newest `<config>/projects/*/<session id>.jsonl`, or where it
/// will be for a session that has not written one yet, under the project folder for `cwd`.
pub fn locate_transcript(config_dir: &Path, session_id: &str, cwd: &str) -> PathBuf {
    let candidates: Vec<(PathBuf, SystemTime)> = transcript_files(config_dir, Some(session_id))
        .into_iter()
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();
    transcript_path(session_id, cwd, config_dir, &candidates)
}

/// The piece of the transcript read at a time, from the end.
const CHUNK: u64 = 256 * 1024;
/// A transcript's newest usage is normally in its last few kilobytes; give up past this.
const MAX_SCAN: u64 = 8 * 1024 * 1024;

/// How many tokens the session's last model call read, from the newest main-chain assistant entry with a
/// usage in the transcript at `path`, or `None` without one. Reads backwards from the end in chunks.
pub fn context_tokens(path: &Path) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let end = file.seek(SeekFrom::End(0)).ok()?;
    let mut position = end;
    let mut tail: Vec<u8> = Vec::new();
    while position > 0 && end - position < MAX_SCAN {
        let step = CHUNK.min(position);
        position -= step;
        file.seek(SeekFrom::Start(position)).ok()?;
        let mut chunk = vec![0u8; step as usize];
        file.read_exact(&mut chunk).ok()?;
        chunk.extend_from_slice(&tail);
        tail = chunk;
        // The first piece may be a partial line unless the file's start was reached.
        let (complete, rest): (Vec<&[u8]>, Vec<u8>) = {
            let mut lines: Vec<&[u8]> = tail.split(|&b| b == b'\n').collect();
            if position == 0 {
                (lines, Vec::new())
            } else {
                let first = lines.remove(0).to_vec();
                (lines, first)
            }
        };
        for line in complete.iter().rev() {
            if !contains(line, b"\"usage\"") {
                continue;
            }
            let Ok(entry) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if let Some(tokens) = usage_tokens(&entry) {
                return Some(tokens);
            }
        }
        tail = rest;
    }
    None
}

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use budd_core::transcript::TranscriptFollower;
    use std::io::Write;

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/2.1.316/transcripts");

    fn fixture(name: &str) -> PathBuf {
        Path::new(FIXTURES).join(name)
    }

    #[test]
    fn context_is_the_newest_main_chain_usage() {
        assert_eq!(context_tokens(&fixture("bg-haiku-f754f44d.jsonl")), Some(47_422));
        assert_eq!(context_tokens(&fixture("bg-haiku-cc7d868c.jsonl")), Some(48_304));
        assert_eq!(context_tokens(&fixture("missing.jsonl")), None);
        let temp = tempfile::tempdir().unwrap();
        let empty = temp.path().join("empty.jsonl");
        fs::write(&empty, "").unwrap();
        assert_eq!(context_tokens(&empty), None);
        // A newer entry with zero usage does not hide the real count, and a sidechain's does not count.
        let mixed = temp.path().join("mixed.jsonl");
        fs::write(
            &mixed,
            concat!(
                "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":5}}}\n",
                "{\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"usage\":{\"input_tokens\":999}}}\n",
                "{\"type\":\"assistant\",\"message\":{\"usage\":{\"input_tokens\":0}}}\n",
                "not json with \"usage\" in it\n",
            ),
        )
        .unwrap();
        assert_eq!(context_tokens(&mixed), Some(15));
    }

    #[test]
    fn context_is_found_across_chunk_edges() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("big.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            "{{\"type\":\"assistant\",\"message\":{{\"usage\":{{\"input_tokens\":4242}}}}}}"
        )
        .unwrap();
        // Three chunks' worth of lines without a usage after it, each line straddling chunk edges.
        let filler = format!(
            "{{\"type\":\"user\",\"message\":{{\"content\":\"{}\"}}}}",
            "x".repeat(100_000)
        );
        for _ in 0..9 {
            writeln!(file, "{filler}").unwrap();
        }
        drop(file);
        assert_eq!(context_tokens(&path), Some(4242));
    }

    #[test]
    fn the_file_source_reads_by_range_and_follows_growth() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        fs::write(&path, "abc\ndef\n").unwrap();
        let source = FsTranscriptSource::new(&path);
        assert_eq!(source.len(), 8);
        assert_eq!(source.read(4..8), b"def\n");
        assert_eq!(source.read(6..100), b"f\n");
        assert_eq!(source.read(5..5), b"");
        assert_ne!(source.identity(), 0);
        let missing = FsTranscriptSource::new(temp.path().join("none.jsonl"));
        assert_eq!(missing.len(), 0);
        assert_eq!(missing.read(0..10), b"");

        let transcript = FsTranscriptSource::new(fixture("bg-haiku-f754f44d.jsonl"));
        let mut follower = TranscriptFollower::new();
        follower.update(&transcript);
        assert_eq!(follower.summary().context, Some(47_422));
        assert!(follower.summary().prompt.is_some());
    }

    #[test]
    fn the_store_lists_the_projects_transcripts_and_finds_the_newest() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path();
        let old = config.join("projects").join("-Users-alice-one");
        let new = config.join("projects").join("-Users-alice-two");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::copy(fixture("bg-haiku-f754f44d.jsonl"), old.join("sid.jsonl")).unwrap();
        fs::copy(fixture("bg-haiku-cc7d868c.jsonl"), new.join("sid.jsonl")).unwrap();
        fs::write(new.join("notes.txt"), "not a transcript").unwrap();
        // `fs::copy` keeps the fixture's own time on some platforms (macOS copies metadata), so both
        // times are set by hand: the new one now, the old one an hour before.
        let now = SystemTime::now();
        let then = now - std::time::Duration::from_secs(3600);
        for (path, at) in [(old.join("sid.jsonl"), then), (new.join("sid.jsonl"), now)] {
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(at)
                .unwrap();
        }

        assert_eq!(
            locate_transcript(config, "sid", "/Users/alice/one"),
            new.join("sid.jsonl")
        );
        // Without a file yet, where the project folder for cwd would put it.
        assert_eq!(
            locate_transcript(config, "fresh", "/Users/alice/three"),
            config.join("projects").join("-Users-alice-three").join("fresh.jsonl")
        );
        assert_eq!(
            locate_transcript(&config.join("nowhere"), "fresh", "/x"),
            config.join("nowhere").join("projects").join("-x").join("fresh.jsonl")
        );

        let store = FsTranscriptStore::new(config);
        let mut candidates = store.candidates();
        candidates.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|c| c.path.ends_with("sid.jsonl")));
        assert!(candidates.iter().all(|c| c.head.len() <= TranscriptChain::HEAD_BYTES));
        assert_eq!(TranscriptChain::name_of(&candidates[0].head).as_deref(), Some("spike0"));
        assert!(store.open(&candidates[0].path).is_some());
        assert!(store.open("/nowhere/none.jsonl").is_none());
        assert_eq!(FsTranscriptStore::new(config.join("nowhere")).candidates().len(), 0);
    }
}
