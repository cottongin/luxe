use super::*;
use serde_json::json;

/// A hand-rolled scanner over half a message deserves the whole truth table.
///
/// Cut a real argument object at every character in it. Whatever survives the cut has to be
/// *what was actually sent* — same keys, same values, byte for byte — because a header that
/// shows a truncated path and then corrects itself is worse than one that waited. Braces,
/// commas and quotes inside string values are the traps, so they are all in here.
#[test]
fn every_cut_through_a_call_leaves_only_what_was_really_sent() {
    let calls = [
        json!({"path": "src/été.rs", "content": "fn main() {}\n\"quoted\", too\\"}),
        json!({"command": "grep -r '{,}' .", "timeout": 30}),
        json!({"path": "a.rs", "edits": [{"oldText": "a,b"}, {"oldText": "c"}], "all": true}),
        json!({"queries": ["one", "two"], "nested": {"a": [1, 2]}, "last": null}),
        // Keys sort `all` first, so a member closes *before* the commas, braces and escaped
        // quotes buried in the string after it — which is what makes a scan that mistakes any
        // of them for structure visibly lose a label it already had.
        json!({"all": true, "command": "echo \"a, b\" # }"}),
        json!({}),
    ];

    for arguments in calls {
        let whole = arguments.to_string();
        let sent = arguments.as_object().expect("an argument object");
        let mut deepest = 0;
        let mut answered_early = false;
        let mut answered = false;
        for (end, _) in whole.char_indices().chain(std::iter::once((whole.len(), ' '))) {
            let cut = PartialObject(&whole[..end]).complete_members();
            // Once a member has closed, the label exists — and every further byte the model
            // sends can only add to it. A cut that goes quiet again is a comma the scan
            // mistook for the end of a member: one inside a string, or inside a nested value.
            assert!(
                cut.is_some() || !answered,
                "cut at {end} lost a label it already had: {:?}",
                &whole[..end],
            );
            answered |= cut.is_some();
            let Some(arrived) = cut else {
                continue;
            };
            let arrived = arrived.as_object().expect("an object in, an object out");
            for (key, value) in arrived {
                assert_eq!(
                    sent.get(key),
                    Some(value),
                    "cut at {end} invented {key}={value} out of {:?}",
                    &whole[..end],
                );
            }
            deepest = deepest.max(arrived.len());
            answered_early |= !arrived.is_empty() && arrived.len() < sent.len();
        }
        assert_eq!(deepest, sent.len(), "the untruncated call has to read back whole");
        // And it answers *before* the end, which is the only reason any of this exists.
        assert_eq!(answered_early, sent.len() > 1, "{whole}");
    }
}
