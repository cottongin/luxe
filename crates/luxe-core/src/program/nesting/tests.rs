use super::*;

/// The depth a human launches at, and one level of delegation under it, always run.
#[test]
fn ordinary_nesting_is_allowed_up_to_the_ceiling() {
    for depth in 0..=DEFAULT_MAX_NESTING_DEPTH {
        assert!(check_nesting(depth, DEFAULT_MAX_NESTING_DEPTH).is_ok(), "depth {depth}");
    }
}

/// Past the ceiling it refuses, and says how to get past it deliberately.
///
/// The refusal is the whole feature: nesting is multiplicative, so the level after the last
/// allowed one is where a delegating agent turns into a fork bomb.
#[test]
fn nesting_past_the_ceiling_is_refused_with_a_way_out() {
    let refused = check_nesting(DEFAULT_MAX_NESTING_DEPTH + 1, DEFAULT_MAX_NESTING_DEPTH)
        .expect_err("one past the ceiling is refused");
    assert_eq!(refused.depth, DEFAULT_MAX_NESTING_DEPTH + 1);
    let said = refused.to_string();
    assert!(said.contains(MAX_NESTING_DEPTH_VAR), "names the way out: {said}");
    assert!(said.contains(NESTING_DEPTH_VAR), "names the stale variable: {said}");
}

/// A ceiling of 0 means "never nest", which must actually mean it.
#[test]
fn a_ceiling_of_zero_forbids_every_nested_luxe() {
    assert!(check_nesting(0, 0).is_ok(), "the human's own session always runs");
    assert!(check_nesting(1, 0).is_err());
}

/// The load-bearing half: a child has to *see* a deeper number, or the guard counts to one
/// forever and never fires. Nothing recognizes that a spawned command is Luxe — an argv cannot
/// tell you that — so inheritance is the entire mechanism.
///
/// Mutates the process environment, which is sound here because nextest gives each test its
/// own process, and no other test reads these variables.
#[test]
fn a_child_inherits_the_next_depth_down() {
    std::env::remove_var(NESTING_DEPTH_VAR);
    std::env::remove_var(MAX_NESTING_DEPTH_VAR);

    assert_eq!(claim_nesting_slot(), Ok(0), "an unset variable means a human launched us");
    assert_eq!(std::env::var(NESTING_DEPTH_VAR).as_deref(), Ok("1"));

    // What that child then does when it starts.
    assert_eq!(claim_nesting_slot(), Ok(1));
    assert_eq!(std::env::var(NESTING_DEPTH_VAR).as_deref(), Ok("2"));
}
