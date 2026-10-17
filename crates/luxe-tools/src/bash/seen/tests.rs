use super::*;

/// A project holding the names a careless parse would count: files named like flags, like
/// standard input, and with shell operators in them. Each exists, so a wrong answer has
/// something to land on rather than being filtered out as missing.
fn project(extra: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    let base =
        ["src/cli.js", "src/filter.js", "test/filter.test.js", "a.txt", "b.txt", "-n", "-40"];
    for path in base.iter().chain(extra) {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, "x\n").expect("write");
    }
    dir
}

fn shown(command: &str, dir: &Path) -> Vec<String> {
    // Joined by component rather than by replacing `\`, which is a legal character in a name
    let relative = |path: &PathBuf| {
        let inside = path.strip_prefix(dir).expect("inside");
        let parts: Vec<_> = inside.iter().map(|part| part.to_string_lossy()).collect();
        parts.join("/")
    };
    files_shown(command, dir, "x\n").iter().map(relative).collect()
}

/// The opening move this exists for: look at a few files, then edit them.
#[test]
fn a_cat_of_files_and_globs_counts() {
    let dir = project(&[]);
    assert_eq!(
        shown("cat src/*.js; ls test; head -40 test/*filter*", dir.path()),
        ["src/cli.js", "src/filter.js", "test/filter.test.js"]
    );
    assert_eq!(shown("cat a.txt && tail -n 5 src/cli.js", dir.path()), ["a.txt", "src/cli.js"]);
    assert_eq!(shown("cat a.txt || cat b.txt", dir.path()), ["a.txt", "b.txt"]);
    assert_eq!(shown("cat a.txt\nnl -ba b.txt", dir.path()), ["a.txt", "b.txt"]);
    assert_eq!(shown("/bin/cat 'src/cli.js'", dir.path()), ["src/cli.js"]);
    assert_eq!(shown("cat a.txt ${UNSET}", dir.path()), ["a.txt"], "a braced variable is a word");
}

/// Output the model never saw as printed counts for nothing.
#[test]
fn piped_redirected_or_backgrounded_output_does_not_count() {
    let dir = project(&[]);
    for command in [
        "cat a.txt | grep foo",
        "cat a.txt > /tmp/copy",
        "cat a.txt >> notes",
        "cat a.txt < b.txt",
        "cat a.txt &",
        "cat a.txt $(true)",
        "cat a.txt `true`",
        "cat a.txt \"$(true)\"",
        "cat a.txt \"`true`\"",
        "cat a.txt 'unterminated",
        "cat > a.txt <<'E'\nnew\nE",
    ] {
        assert!(shown(command, dir.path()).is_empty(), "{command}");
    }
    for command in ["cat a.txt >&2", "cat a.txt 1>out", "cat a.txt &>out", "cat a.txt 12>x"] {
        assert!(shown(command, dir.path()).is_empty(), "stdout may be hidden: {command}");
    }
    // A pipe in one segment leaves the others alone
    assert_eq!(shown("cat a.txt; cat b.txt | wc -l", dir.path()), ["a.txt"]);
}

/// Piped into `head` or `tail`, a file is still shown, a slice of it the way `read` with a
/// `limit` shows one. Into anything that changes the text, it is not.
#[test]
fn a_pipe_into_head_or_tail_is_a_slice() {
    let dir = project(&[]);
    assert_eq!(shown("cat a.txt | head -60", dir.path()), ["a.txt"]);
    // Sending stderr away hides nothing the model reads
    assert_eq!(
        shown("cat test/*filter* 2>/dev/null | head -40", dir.path()),
        ["test/filter.test.js"]
    );
    assert_eq!(shown("cat a.txt 2>&1 | head -5", dir.path()), ["a.txt"]);
    assert_eq!(shown("cat a.txt 2> /dev/null", dir.path()), ["a.txt"]);
    assert_eq!(shown("cat test/*.js | head -80", dir.path()), ["test/filter.test.js"]);
    assert_eq!(shown("nl a.txt | head -n 5 | tail -n 2", dir.path()), ["a.txt"]);
    for command in [
        "cat a.txt | head | sort",
        "cat a.txt | head -5 b.txt",
        "cat a.txt | tail -f",
        "cat a.txt | tail -F",
        "cat a.txt | tail --follow",
        "cat a.txt | head -5 > out.txt",
        "cat a.txt |",
        "| head",
        "grep x a.txt | sort",
    ] {
        assert!(shown(command, dir.path()).is_empty(), "{command}");
    }
}

/// Operators inside quotes, or escaped, are part of a name; outside them they split or taint.
#[cfg(unix)]
#[test]
fn quoted_and_escaped_operators_are_names() {
    let dir = project(&["a|b.txt", "a;b.txt", "a>b.txt", "it\"s.txt", "it's.txt", "x\\"]);
    assert_eq!(shown("cat 'a|b.txt'", dir.path()), ["a|b.txt"]);
    assert_eq!(shown("cat \"a;b.txt\"", dir.path()), ["a;b.txt"]);
    assert_eq!(shown("cat 'a>b.txt' a.txt", dir.path()), ["a>b.txt", "a.txt"]);
    assert_eq!(shown("cat a\\|b.txt", dir.path()), ["a|b.txt"]);
    assert_eq!(shown("cat 'it\"s.txt'", dir.path()), ["it\"s.txt"]);
    assert_eq!(shown("cat \"it's.txt\"", dir.path()), ["it's.txt"]);
    // A backslash in single quotes is a character, so the quote still closes at the next `'`
    assert_eq!(shown("cat 'x\\' ; true", dir.path()), ["x\\"]);
    assert_eq!(shown("cat '$(true)' a.txt", dir.path()), ["a.txt"], "single quotes run nothing");
}

/// Only the printers count, and only the forms of them that print.
#[test]
fn only_plain_printers_count() {
    let dir = project(&[]);
    for command in [
        "sed -i 's/a/b/' a.txt",
        "sed --in-place -n '1p' a.txt",
        "sed 's/a/b/' a.txt",
        "sed -n -e '1p' a.txt",
        "sed -n -f script.sed a.txt",
        "tail -f a.txt",
        "tail -F a.txt",
        "tail --follow a.txt",
        "grep -l foo a.txt",
        "grep -c foo a.txt",
        "grep -qn foo a.txt",
        "rg --files-with-matches foo a.txt",
        "grep -o foo a.txt",
        "less a.txt",
        "rm a.txt",
        "echo 'cat a.txt'",
    ] {
        assert!(shown(command, dir.path()).is_empty(), "{command}");
    }
    assert!(shown("cat missing.txt", dir.path()).is_empty(), "not a file, not a read");
}

/// `sed -n` prints the files after its script, whichever spelling of quiet it was given.
#[test]
fn sed_counts_the_files_after_its_script() {
    let dir = project(&["1p"]);
    assert_eq!(shown("sed -n '1,40p' src/filter.js", dir.path()), ["src/filter.js"]);
    assert_eq!(shown("sed --quiet 1p a.txt", dir.path()), ["a.txt"]);
    assert_eq!(shown("sed --silent 1p a.txt", dir.path()), ["a.txt"]);
    // The script is a script even when a file shares its name
    assert_eq!(shown("sed -n 1p b.txt", dir.path()), ["b.txt"]);
}

/// A search shows the lines it matched, which is a slice of the file like `sed -n` prints.
#[test]
fn a_search_that_prints_lines_counts_its_file() {
    let dir = project(&[]);
    assert_eq!(shown("grep -n -A8 'def x' a.txt", dir.path()), ["a.txt"]);
    assert_eq!(shown("grep -n x a.txt | head -40", dir.path()), ["a.txt"]);
    assert_eq!(shown("rg -C3 x src/cli.js", dir.path()), ["src/cli.js"]);
    assert_eq!(shown("grep -e a.txt b.txt", dir.path()), ["b.txt"], "a.txt is the pattern");
    assert_eq!(shown("grep -A 3 x b.txt", dir.path()), ["b.txt"], "3 is -A's value");
    assert_eq!(shown("grep --context=3 x b.txt", dir.path()), ["b.txt"]);
    assert_eq!(shown("grep a.txt b.txt", dir.path()), ["b.txt"], "the first word is the pattern");
    // Nothing printed, nothing seen
    assert!(files_shown("grep x a.txt", dir.path(), "").is_empty());
}

/// A search over many files counts the files its rows name, matches and context alike.
#[test]
fn a_search_over_many_files_counts_the_files_it_printed() {
    let dir = project(&["src/a-1.js"]);
    let printed = "src/cli.js:12:x\nsrc/a-1.js-3-context\nsrc/a-1.js:4:x\nnot/a/file.js:1:x\n";
    let rows = files_shown("grep -rn x src", dir.path(), printed);
    let names: Vec<_> =
        rows.iter().map(|p| p.strip_prefix(dir.path()).unwrap().to_owned()).collect();
    assert_eq!(names, [PathBuf::from("src/cli.js"), PathBuf::from("src/a-1.js")]);
    // A printer's own output is never read as rows
    assert!(files_shown("cat missing", dir.path(), printed).is_empty());
}

/// Flags and their values are not files, `--` ends the flags, and `-` is standard input.
#[test]
fn flags_are_not_files() {
    let dir = project(&["-"]);
    assert_eq!(shown("head -n 20 -q a.txt", dir.path()), ["a.txt"]);
    assert_eq!(shown("head -n a.txt b.txt", dir.path()), ["b.txt"], "a.txt is -n's value");
    assert_eq!(shown("tail -40 src/filter.js", dir.path()), ["src/filter.js"]);
    assert_eq!(shown("cat -n -- src/cli.js", dir.path()), ["src/cli.js"]);
    assert_eq!(shown("cat -- -n", dir.path()), ["-n"], "past `--`, a dash is a name");
    assert_eq!(shown("cat - a.txt", dir.path()), ["a.txt"]);
    assert_eq!(shown("cat -- - a.txt", dir.path()), ["a.txt"], "still standard input");
}
