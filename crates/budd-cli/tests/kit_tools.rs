//! The kit's tools through the real `budd` binary, as the skills call them: the arguments, the printed
//! lines and the exit codes. The modules behind them have their own tests; this checks the wiring.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BUDD: &str = env!("CARGO_BIN_EXE_budd");

struct Bench {
    _temp: tempfile::TempDir,
    home: PathBuf,
    repo: PathBuf,
}

impl Bench {
    /// A Budd home and a git repository with one commit, or `None` when git is not there.
    fn new() -> Option<Bench> {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let home = root.join("home");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !git(&["init", "-q", "-b", "main"]) {
            eprintln!("skipping: git is not available");
            return None;
        }
        assert!(git(&["config", "user.email", "t@example.test"]));
        assert!(git(&["config", "user.name", "Test"]));
        assert!(git(&["config", "commit.gpgsign", "false"]));
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        assert!(git(&["add", "-A"]));
        assert!(git(&["commit", "-q", "-m", "first"]));
        Some(Bench {
            _temp: temp,
            home,
            repo,
        })
    }

    /// Runs `budd <args>` in the repository, with `BUDD_HOME` set and no `CLAUDE_CONFIG_DIR`.
    fn budd(&self, args: &[&str], stdin: Option<&str>) -> Output {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new(BUDD)
            .args(args)
            .current_dir(&self.repo)
            .env("BUDD_HOME", &self.home)
            .env_remove("CLAUDE_CONFIG_DIR")
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = stdin {
            child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        }
        child.wait_with_output().unwrap()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.repo.join(name)
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

#[test]
fn the_do_it_lane_end_to_end() {
    let Some(bench) = Bench::new() else { return };
    let document = bench.path("CASBO-1-expectations.md");
    std::fs::write(
        &document,
        "Problems:\n- The header is wrong.\nExpected:\n- It reads Bet Date/Time.\n- The CSV matches.\nPotential risk:\n- The export reads it too.\n",
    )
    .unwrap();
    let document = document.to_str().unwrap();

    // expect check, plain and --json.
    let out = bench.budd(&["expect", "check", document], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "ok: 1 problems, 2 expected, 1 risk\n");
    let out = bench.budd(&["expect", "check", document, "--json"], None);
    assert_eq!(text(&out.stdout), "[]\n");
    let bad = bench.path("bad.md");
    std::fs::write(&bad, "Problems:\n- One. Two.\n").unwrap();
    let out = bench.budd(&["expect", "check", bad.to_str().unwrap()], None);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stdout).starts_with("FAIL line 2 (Problems): 2 sentences; a bullet is ONE sentence\n"));
    let out = bench.budd(&["expect", "check", bad.to_str().unwrap(), "--json"], None);
    assert_eq!(code(&out), 1);
    let findings: Vec<String> = serde_json::from_str(&text(&out.stdout)).unwrap();
    assert_eq!(findings.len(), 3, "{findings:?}");
    let out = bench.budd(&["expect", "check", "missing.md"], None);
    assert_eq!(code(&out), 2);

    // handover home names the folder under the git dir; the sidecar goes there before any section.
    let out = bench.budd(&["handover", "home", "--repo", "."], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let ho = PathBuf::from(text(&out.stdout).trim_end());
    assert_eq!(ho, bench.repo.join(".git").join("budd-handover"));
    assert!(!ho.exists());
    let sidecar = ho.join("expectations.json");
    let out = bench.budd(
        &[
            "expect",
            "sidecar",
            document,
            "--ticket",
            "CASBO-1",
            "--out",
            sidecar.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert!(text(&out.stdout).ends_with(&format!("sidecar: {} (E1..E2)\n", sidecar.display())));
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&sidecar).unwrap()).unwrap();
    assert_eq!(written["ticket"], "CASBO-1");
    assert_eq!(written["acceptance"][1]["id"], "E2");

    // evidence, under <home>/claude since CLAUDE_CONFIG_DIR is not set.
    let out = bench.budd(
        &[
            "evidence",
            "init",
            "CASBO-1",
            "--expectations",
            sidecar.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let set = bench.home.join("claude").join("evidence").join("CASBO-1");
    assert_eq!(
        text(&out.stdout),
        format!("evidence: CASBO-1 has 2 expectations at {}\n", set.display())
    );
    assert!(set.join("evidence.json").is_file());
    let out = bench.budd(&["evidence", "check", "CASBO-1"], None);
    assert_eq!(code(&out), 1);
    assert!(
        text(&out.stdout).ends_with("evidence CASBO-1: 4 gap(s)\n"),
        "{}",
        text(&out.stdout)
    );
    let shot = bench.path("shot.png");
    std::fs::write(&shot, "png").unwrap();
    for args in [
        vec!["evidence", "screen", "CASBO-1", "E1", "yes"],
        vec![
            "evidence",
            "add",
            "CASBO-1",
            "E1",
            "before",
            shot.to_str().unwrap(),
            "--url",
            "http://x",
            "--steps",
            "search",
        ],
        vec!["evidence", "add", "CASBO-1", "E1", "after", shot.to_str().unwrap()],
        vec!["evidence", "screen", "CASBO-1", "E2", "no"],
        vec!["evidence", "proof", "CASBO-1", "E2", "csv.spec.js"],
    ] {
        let out = bench.budd(&args, None);
        assert_eq!(code(&out), 0, "{args:?}: {}", text(&out.stderr));
    }
    let out = bench.budd(&["evidence", "check", "CASBO-1"], None);
    assert_eq!(code(&out), 0);
    assert_eq!(text(&out.stdout), "evidence CASBO-1: complete\n");
    let out = bench.budd(&["evidence", "show", "CASBO-1"], None);
    assert!(text(&out.stdout).contains("- E1: before yes / after yes\n- E2: no screen; proof: csv.spec.js\n"));
    let out = bench.budd(
        &[
            "evidence", "shoot", "--ticket", "CASBO-1", "--id", "E1", "--when", "before",
        ],
        None,
    );
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).starts_with("evidence: evidence shots are not installed\n    budd evidence init"));
    let out = bench.budd(&["evidence", "check", "../etc"], None);
    assert_eq!(code(&out), 2);
    // A config dir of its own wins over the home.
    let other = bench.home.join("other-config");
    let out = Command::new(BUDD)
        .args(["evidence", "check", "CASBO-1"])
        .env("BUDD_HOME", &bench.home)
        .env("CLAUDE_CONFIG_DIR", &other)
        .output()
        .unwrap();
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("no evidence set for CASBO-1"));

    // handover append from stdin and from a file, show, tokens.
    let out = bench.budd(
        &[
            "handover", "append", "--repo", ".", "--skill", "do-it", "--title", "cause",
        ],
        Some("Cause `a.txt:1` - wrong.\n"),
    );
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim_end(), ho.join("HANDOVER.md").to_str().unwrap());
    let out = bench.budd(&["handover", "append", "expectations", "--from", document], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let out = bench.budd(&["handover", "append", "empty"], Some("  \n"));
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("empty body"));
    let out = bench.budd(&["handover", "show"], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.starts_with("Live now:  branch=main  head="), "{shown}");
    assert!(shown.contains("[CURRENT   ] ## do-it - cause\n"));
    assert!(shown.contains("[CURRENT   ] ## expectations\n"));
    assert!(shown.contains("| tokens in | not recorded (no --session passed) |"));
    let out = bench.budd(&["handover", "show", "cause"], None);
    assert!(text(&out.stdout).contains("## do-it - cause") && !text(&out.stdout).contains("## expectations"));
    let out = bench.budd(&["handover", "tokens", "no-such-session"], None);
    assert_eq!(code(&out), 0);
    assert_eq!(
        text(&out.stdout),
        "in:      not recorded (transcript not found)\nout:     not recorded\ncontext: not recorded\nsession: no-such- transcript not found\n"
    );
    let elsewhere = bench.home.join("not-a-repo");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let out = bench.budd(&["handover", "show", "--worktree", elsewhere.to_str().unwrap()], None);
    assert_eq!(code(&out), 2);
    assert!(text(&out.stderr).contains("not a git repository"));

    // fingerprint: the oid, stable, moved by an edit, unmoved by the handover.
    let out = bench.budd(&["fingerprint"], None);
    assert_eq!(code(&out), 0, "{}", text(&out.stderr));
    let first = text(&out.stdout);
    assert_eq!(first.trim_end().len(), 40);
    let out = bench.budd(&["fingerprint", bench.repo.to_str().unwrap()], None);
    assert_eq!(text(&out.stdout), first);
    std::fs::write(bench.path("a.txt"), "two\n").unwrap();
    let out = bench.budd(&["fingerprint"], None);
    assert_ne!(text(&out.stdout), first);
    let out = bench.budd(&["fingerprint", elsewhere.to_str().unwrap()], None);
    assert_eq!(code(&out), 1);
    assert!(text(&out.stderr).starts_with("budd fingerprint: "));
    assert!(
        Path::new(&bench.repo)
            .join(".git")
            .join("budd-handover")
            .join("HANDOVER.md")
            .is_file()
    );
}
