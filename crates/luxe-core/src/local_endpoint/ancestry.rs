//! Ancestry — whether a process was started, at any depth, by another.
//!
//! The tripwire that keeps a luxe's own children (the agent's `bash`, a hook) from joining its
//! session and answering their own prompts. A tripwire, not a wall: a process that detaches itself
//! leaves the chain, and the documentation says so. Blocking: run it off the async runtime.

/// How many parents to walk before giving up; real chains are a handful deep.
const MAX_DEPTH: usize = 64;

/// Whether `pid` is `ancestor` or was started, at any depth, by it. `false` when the chain cannot
/// be read, so a client this cannot place is let in, as it would be without the tripwire.
pub fn descends_from(pid: u32, ancestor: u32) -> bool {
    descends_in(pid, ancestor, parent_lookup())
}

/// [`descends_from`] over an explicit parent table.
fn descends_in(pid: u32, ancestor: u32, parent_of: impl Fn(u32) -> Option<u32>) -> bool {
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if current == ancestor {
            return true;
        }
        match parent_of(current) {
            Some(parent) if parent != current && parent != 0 => current = parent,
            _ => return false,
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn parent_lookup() -> impl Fn(u32) -> Option<u32> {
    |pid| parent_in_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The parent id from a `/proc/<pid>/stat` line. The command name sits in parentheses and may hold
/// spaces and parentheses itself, so fields are counted from the last `)`.
#[cfg(any(target_os = "linux", test))]
fn parent_in_stat(stat: &str) -> Option<u32> {
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(all(unix, not(target_os = "linux")))]
fn parent_lookup() -> impl Fn(u32) -> Option<u32> {
    let table = process_table();
    move |pid| table.get(&pid).copied()
}

/// Every process's parent, from `ps`, which macOS and the BSDs all have at the same path.
#[cfg(all(unix, not(target_os = "linux")))]
fn process_table() -> std::collections::HashMap<u32, u32> {
    let listing = std::process::Command::new("/bin/ps").args(["-A", "-o", "pid=,ppid="]).output();
    let Ok(listing) = listing else {
        return std::collections::HashMap::new();
    };
    String::from_utf8_lossy(&listing.stdout)
        .lines()
        .filter_map(|line| {
            let mut ids = line.split_whitespace().map(str::parse::<u32>);
            Some((ids.next()?.ok()?, ids.next()?.ok()?))
        })
        .collect()
}

#[cfg(windows)]
fn parent_lookup() -> impl Fn(u32) -> Option<u32> {
    let table = super::windows::process_parents();
    move |pid| table.get(&pid).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process is its own descendant, a grandchild is found through its parent, and an
    /// unrelated process or a broken chain is not.
    #[test]
    fn the_chain_is_walked_to_the_ancestor_and_no_further() {
        let parents = |pid| match pid {
            30 => Some(20),
            20 => Some(10),
            10 => Some(1),
            1 => Some(0),
            _ => None,
        };
        assert!(descends_in(30, 10, parents), "a grandchild");
        assert!(descends_in(10, 10, parents), "itself");
        assert!(!descends_in(10, 30, parents), "a parent is not a child");
        assert!(!descends_in(99, 10, parents), "unknown, so not placed");
        assert!(!descends_in(7, 10, |_| Some(7)), "a process that is its own parent ends the walk");
    }

    /// The parent is the fourth field, counted after a command name that can hold anything.
    #[test]
    fn the_parent_is_read_past_an_awkward_command_name() {
        assert_eq!(parent_in_stat("4242 (bash) S 4000 4242 4242 0"), Some(4000));
        assert_eq!(parent_in_stat("4242 (my (odd) prog) R 17 4242"), Some(17));
        assert_eq!(parent_in_stat("garbage"), None);
    }

    /// A real child of this test is found, and this test is not the child's.
    #[test]
    #[cfg(unix)]
    fn a_real_child_descends_from_this_process() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().expect("sleep runs");
        let (child_id, own_id) = (child.id(), std::process::id());
        let found = descends_from(child_id, own_id);
        let reversed = descends_from(own_id, child_id);
        let _ = child.kill();
        let _ = child.wait();
        assert!(found, "the child is placed under this process");
        assert!(!reversed);
    }
}
