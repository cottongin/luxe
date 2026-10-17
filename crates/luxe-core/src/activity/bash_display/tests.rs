use super::*;

/// The rule for dropping a `cd` that goes where the shell already is.
///
/// Stated against an explicit root so it is a pure function under test, which is the half that
/// can be got wrong; the one line that asks where that root *is* has nothing to break.
#[test]
fn a_cd_into_the_directory_bash_already_starts_in_is_dropped() {
    let root = Path::new("/home/dev/project");
    let same = |command: &str, expected: &str| {
        assert_eq!(strip_redundant_cd(command, root), expected, "{command}");
    };

    same("cd /home/dev/project && cargo build", "cargo build");
    same("cd /home/dev/project/ && cargo build", "cargo build");
    same("cd '/home/dev/project' && ls", "ls");
    same("cd \"/home/dev/project\" && ls", "ls");
    same("cd /home/dev/project; ls", "ls");
    same("  cd /home/dev/project   &&   ls -la", "ls -la");
    same("cd . && ls", "ls");
    same("cd ./ && ls", "ls");
    // The separator is not part of the path, spaced or not.
    same("cd /home/dev/project;ls", "ls");
    same("cd /home/dev/project&&ls", "ls");
    // Only the one prefix: a second `cd` is a real move and stays.
    same("cd /home/dev/project && cd web && npm run dev", "cd web && npm run dev");
}

/// Everything the rule must keep, because a `cd` that is not a no-op is load-bearing and a card
/// that hides it describes a command that never ran.
#[test]
fn a_cd_that_goes_anywhere_else_is_left_alone() {
    let root = Path::new("/home/dev/project");
    let kept = |command: &str| {
        assert_eq!(strip_redundant_cd(command, root), command, "{command}");
    };

    kept("cd web && npm run dev");
    kept("cd /home/dev/other && cargo build");
    kept("cd /home/dev/project-two && cargo build");
    kept("cd /home/dev && cargo build");
    kept("cd /home/dev/project/web && npm test");
    // Nothing the shell would have to expand, because this is not a shell.
    kept("cd $PROJECT && ls");
    kept("cd ~/project && ls");
    kept("cd \"$(git rev-parse --show-toplevel)\" && ls");
    // Not a sequencing operator, so not this shape.
    kept("cd /home/dev/project | tee log");
    kept("cd /home/dev/project & ls");
    // Nothing would be left to show.
    kept("cd /home/dev/project");
    kept("cd /home/dev/project &&");
    // A different command that merely starts with the same two letters.
    kept("cdate /home/dev/project && ls");
    kept("cargo build");
    // A `cd` with no argument goes home, which is a real move and rarely the project root.
    kept("cd && ls");
    kept("cd \"\" && ls");
    kept("cd '' && ls");
}
