//! Remote — shared pieces of the pull request scenarios.

use std::path::{Path, PathBuf};

pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

pub(crate) fn commit(dir: &Path, file: &str, contents: &str, subject: &str) -> String {
    std::fs::write(dir.join(file), contents).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "--quiet", "-m", subject]);
    git(dir, &["rev-parse", "HEAD"])
}

pub(crate) struct Remote {
    pub(crate) _dir: tempfile::TempDir,
    pub(crate) seed: PathBuf,
    pub(crate) project: PathBuf,
    pub(crate) base: String,
    pub(crate) head: String,
}

impl Remote {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let origin = dir.path().join("origin.git");
        let seed = dir.path().join("seed");
        let project = dir.path().join("project");
        git(dir.path(), &["init", "--quiet", "--bare", "-b", "main", origin.to_str().unwrap()]);
        git(dir.path(), &["init", "--quiet", "-b", "main", seed.to_str().unwrap()]);
        git(&seed, &["config", "user.name", "Luxe Tests"]);
        git(&seed, &["config", "user.email", "tests@luxe.local"]);
        git(&seed, &["remote", "add", "origin", origin.to_str().unwrap()]);
        let base = commit(&seed, "parser.rs", "fn parse() {}\n", "Start");
        git(&seed, &["push", "--quiet", "origin", "main"]);
        git(dir.path(), &["clone", "--quiet", origin.to_str().unwrap(), project.to_str().unwrap()]);
        // The project commits too, in the tests that commit a review, and a machine with no global
        // identity — a fresh CI runner, Windows under wine — would refuse
        git(&project, &["config", "user.name", "Luxe Tests"]);
        git(&project, &["config", "user.email", "tests@luxe.local"]);
        std::fs::create_dir(project.join(".luxe")).unwrap();
        std::fs::write(project.join(".luxe/.gitignore"), "*\n").unwrap();
        git(&seed, &["switch", "--quiet", "-c", "fix-parser"]);
        let head = commit(&seed, "parser.rs", "fn parse() { keep_last_token(); }\n", "Keep it");
        git(&seed, &["push", "--quiet", "origin", "HEAD:refs/pull/1/head"]);
        Self { _dir: dir, seed, project, base, head }
    }

    pub(crate) fn push(&self, file: &str, contents: &str) -> String {
        let head = commit(&self.seed, file, contents, "More");
        git(&self.seed, &["push", "--quiet", "origin", "HEAD:refs/pull/1/head"]);
        head
    }
}
