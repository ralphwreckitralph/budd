//! The Claude Code versions Budd supports, from `core/compat.toml` at the repository root.
//!
//! Below `minimum` Budd refuses to connect and asks for `claude update`; between `minimum` and `tested`
//! the release's end-to-end suite ran against that very version; above `tested` Budd only notices, as
//! newer versions are expected to keep working until a reader here proves otherwise.

use semver::Version;
use std::sync::LazyLock;

/// The file, compiled in: the gate ships with the binary and never depends on a checkout.
pub const COMPAT_TOML: &str = include_str!("../../../../core/compat.toml");

/// The oldest Claude Code Budd connects to.
pub static SUPPORTED: LazyLock<Version> =
    LazyLock::new(|| parse_field(COMPAT_TOML, "claude", "minimum").expect("core/compat.toml names [claude] minimum"));
/// The newest Claude Code the release's end-to-end suite ran against.
pub static TESTED_THROUGH: LazyLock<Version> =
    LazyLock::new(|| parse_field(COMPAT_TOML, "claude", "tested").expect("core/compat.toml names [claude] tested"));

/// Where a Claude Code version stands with this Budd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Standing {
    /// Older than [`SUPPORTED`]: Budd does not connect.
    Unsupported,
    /// Between [`SUPPORTED`] and [`TESTED_THROUGH`], both included.
    Tested,
    /// Newer than [`TESTED_THROUGH`]: expected to work, worth a notice.
    Untested,
}

impl Standing {
    /// Where `version` stands.
    pub fn of(version: &Version) -> Standing {
        if *version < *SUPPORTED {
            Standing::Unsupported
        } else if *version <= *TESTED_THROUGH {
            Standing::Tested
        } else {
            Standing::Untested
        }
    }
}

/// The version under `key = "x.y.z"` in the `[table]` of `toml`. A whole TOML parser would be more
/// than the two lines the file has; this reads tables, quoted strings and `#` comments and nothing else.
pub fn parse_field(toml: &str, table: &str, key: &str) -> Option<Version> {
    let mut in_table = false;
    for raw in toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            in_table = header.trim() == table;
            continue;
        }
        if !in_table {
            continue;
        }
        let (name, value) = line.split_once('=')?;
        if name.trim() == key {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            return Version::parse(value).ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_reads_the_shipped_file() {
        assert!(*SUPPORTED <= *TESTED_THROUGH);
        assert!(*SUPPORTED >= Version::new(2, 0, 0));
    }

    #[test]
    fn a_version_stands_below_within_or_above() {
        let below = Version::new(SUPPORTED.major, SUPPORTED.minor, SUPPORTED.patch.saturating_sub(1));
        if below < *SUPPORTED {
            assert_eq!(Standing::of(&below), Standing::Unsupported);
        }
        assert_eq!(Standing::of(&Version::new(1, 0, 0)), Standing::Unsupported);
        assert_eq!(Standing::of(&SUPPORTED), Standing::Tested);
        assert_eq!(Standing::of(&TESTED_THROUGH), Standing::Tested);
        let above = Version::new(TESTED_THROUGH.major, TESTED_THROUGH.minor, TESTED_THROUGH.patch + 1);
        assert_eq!(Standing::of(&above), Standing::Untested);
        assert_eq!(Standing::of(&Version::new(99, 0, 0)), Standing::Untested);
    }

    #[test]
    fn the_line_parser_reads_tables_quotes_and_comments() {
        let toml = "# header\n[other]\nminimum = \"9.9.9\"\n\n[claude] # the gate\nminimum = '2.1.300' # inline\ntested=\"2.1.316\"\n";
        assert_eq!(parse_field(toml, "claude", "minimum"), Some(Version::new(2, 1, 300)));
        assert_eq!(parse_field(toml, "claude", "tested"), Some(Version::new(2, 1, 316)));
        assert_eq!(parse_field(toml, "other", "minimum"), Some(Version::new(9, 9, 9)));
        assert_eq!(parse_field(toml, "claude", "newest"), None);
        assert_eq!(parse_field(toml, "missing", "minimum"), None);
        assert_eq!(parse_field("[claude]\nminimum = \"soon\"\n", "claude", "minimum"), None);
    }
}
