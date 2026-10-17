use super::*;
use serde_json::json;

/// The shapes the major harnesses trained models on all arrive here eventually.
#[test]
fn every_dialect_normalizes_to_the_same_request() {
    let luxe = json!({"path": "a.rs", "edits": [{"oldText": "x", "newText": "y"}]});
    let claude_code =
        json!({"file_path": "a.rs", "edits": [{"old_string": "x", "new_string": "y"}]});
    let opencode = json!({"filePath": "a.rs", "edits": [{"oldString": "x", "newString": "y"}]});
    let snake = json!({"path": "a.rs", "edits": [{"old_text": "x", "new_text": "y"}]});

    let expected = EditRequest {
        path: "a.rs".into(),
        edits: vec![EditSpec { old_text: "x".into(), new_text: "y".into(), replace_all: false }],
    };
    for args in [luxe, claude_code, opencode, snake] {
        assert_eq!(normalize(&args).expect("normalizes"), expected, "{args}");
    }
}

/// The rename is only free because the spelling it replaced still normalizes.
///
/// `old_text`/`new_text` was the advertised shape for long enough to be in every transcript a
/// session resumes from, so dropping it from [`OLD_KEYS`]/[`NEW_KEYS`] as "dead" would break
/// exactly the calls the rename was meant to stop costing turns.
#[test]
fn the_spelling_the_schema_used_to_advertise_is_still_accepted() {
    let request = normalize(&json!({
        "path": "a.rs",
        "edits": [{"old_text": "x", "new_text": "y", "replace_all": true}],
    }))
    .expect("the retired spelling still normalizes");
    assert_eq!(request.edits[0].old_text, "x");
    assert_eq!(request.edits[0].new_text, "y");
    assert!(request.edits[0].replace_all);
}

/// Canonical first, so a call mixing dialects resolves to the spelling the schema advertises.
#[test]
fn the_canonical_spelling_wins_when_two_dialects_arrive_together() {
    let request = normalize(&json!({
        "path": "a.rs",
        "edits": [{"oldText": "canonical", "old_string": "alias", "newText": "y"}],
    }))
    .expect("normalizes");
    assert_eq!(request.edits[0].old_text, "canonical");
}

/// A single-edit call is the same call. Claude Code's `Edit` takes exactly one.
#[test]
fn a_single_edit_may_be_passed_at_the_top_level_or_as_a_lone_object() {
    let flat = json!({"file_path": "a.rs", "old_string": "x", "new_string": "y"});
    let object = json!({"path": "a.rs", "edits": {"old_text": "x", "new_text": "y"}});
    for args in [flat, object] {
        let request = normalize(&args).expect("normalizes");
        assert_eq!(request.edits.len(), 1);
        assert_eq!(request.edits[0].old_text, "x");
    }
}

#[test]
fn replace_all_is_read_from_any_of_its_spellings_and_defaults_off() {
    for key in ["replaceAll", "replace_all", "all"] {
        let args = json!({"path": "a.rs", "oldText": "x", "newText": "y", key: true});
        assert!(normalize(&args).expect("normalizes").edits[0].replace_all, "{key}");
    }
    let args = json!({"path": "a.rs", "oldText": "x", "newText": "y"});
    assert!(!normalize(&args).expect("normalizes").edits[0].replace_all);
}

/// Deleting text is `newText: ""`; *omitting* it is a truncated call, and applying that
/// would silently delete the match.
#[test]
fn an_empty_replacement_deletes_but_a_missing_one_is_an_error() {
    let deletion = json!({"path": "a.rs", "oldText": "x", "newText": ""});
    assert_eq!(normalize(&deletion).expect("normalizes").edits[0].new_text, "");

    let truncated = json!({"path": "a.rs", "oldText": "x"});
    assert!(matches!(normalize(&truncated), Err(ArgsError::EditField { field: "newText", .. })));
}

/// An empty `oldText` is refused, and the asymmetry with `newText` is deliberate.
///
/// An empty needle matches at every byte boundary, so `replaceAll` over one would interleave
/// the replacement between every character in the file — a 300 KB source becomes hundreds of
/// megabytes of garbage, and the tool reports success. Making the two fields symmetric is the
/// one-line "cleanup" that turns `edit` into a shredder, so the refusal is pinned rather than
/// left to the helper it happens to live in.
#[test]
fn an_empty_search_is_refused_because_it_would_match_everywhere() {
    assert_eq!(
        "abc".match_indices("").count(),
        4,
        "the primitive this guards against: an empty needle hits every boundary"
    );

    for key in OLD_KEYS {
        let args = json!({"path": "a.rs", key: "", "newText": "y"});
        assert!(
            matches!(normalize(&args), Err(ArgsError::EditField { field: "oldText", .. })),
            "{key} accepted an empty search"
        );
    }

    // Even with the flag that would apply it everywhere.
    let args = json!({"path": "a.rs", "oldText": "", "newText": "y", "replaceAll": true});
    assert!(matches!(normalize(&args), Err(ArgsError::EditField { field: "oldText", .. })));
}

/// One accidental layer of quoting is the same call, and models add it constantly.
#[test]
fn a_json_encoded_edits_array_is_unwrapped_rather_than_refused() {
    let quoted = json!({
        "path": "a.rs",
        "edits": r#"[{"oldText": "x", "newText": "y", "replaceAll": true}]"#,
    });
    let request = normalize(&quoted).expect("a quoted array still parses");
    assert_eq!(
        request.edits,
        vec![EditSpec { old_text: "x".into(), new_text: "y".into(), replace_all: true }]
    );

    // A lone quoted object is a quoted single edit.
    let object = json!({"path": "a.rs", "edits": r#"{"oldText": "x", "newText": "y"}"#});
    assert_eq!(normalize(&object).expect("a quoted object too").edits.len(), 1);
}

/// The failure a real session lost six turns to, minus the hoisted sibling that now lets the
/// whole shape be read.
///
/// The model emitted `edits` as a truncated string (`[{"new_text">…`). `edits` being unusable
/// was the whole story, and neither error it got said so: one claimed there were no edits while
/// listing `edits` among the keys, the other blamed `edit[0]` for a field missing from it. It
/// concluded the tool cared about argument *order* and started permuting keys.
#[test]
fn an_unusable_edits_value_is_named_and_quoted_rather_than_blamed_on_a_sibling() {
    // The same corruption with its sibling missing: neither half to pair it with
    let garbled = json!({
        "path": "src/main.rs",
        "edits": "[{\"new_text\">                    tracing::trace!(?key.code);",
    });
    let error = normalize(&garbled).expect_err("nothing can be read out of that");
    assert!(
        matches!(error, ArgsError::UnusableEdits { key: "edits", found: "a string", .. }),
        "the unusable value is what failed, not the sibling: {error:?}"
    );

    let message = error.message();
    assert!(message.contains("`edits` arrived as a string"), "{message}");
    assert!(message.contains(r#"[{"new_text">"#), "quotes what arrived: {message}");
    assert!(message.contains("Nothing was written"), "{message}");
    assert!(!message.contains("edit[0]"), "never blames an edit that was never sent: {message}");
}

/// Every other type `edits` can arrive as fails the same way, by name.
/// Copied from a claude-sonnet-5-5 run: the search text swallowed into `edits`, the
/// replacement hoisted beside it. Both halves are whole, so it is read as the edit it was.
#[test]
fn the_hoisted_garble_is_read_as_the_one_edit_it_was() {
    let garbled = json!({
        "path": "django/forms/fields.py",
        "edits": "[{\"oldText\">        result.validators = self.validators[:]\n        return result",
        "newText": "        result.validators = self.validators[:]\n        result.error_messages = self.error_messages.copy()\n        return result",
    });
    let request = normalize(&garbled).expect("both halves arrived");
    assert_eq!(request.edits.len(), 1);
    assert_eq!(
        request.edits[0].old_text,
        "        result.validators = self.validators[:]\n        return result"
    );
    assert!(request.edits[0].new_text.contains("error_messages.copy()"));

    // The mirror image, and a key that is no search or replacement at all
    let mirrored = json!({"path": "a.rs", "edits": "[{\"newText\">y", "oldText": "x"});
    let mirrored = normalize(&mirrored).expect("mirror image");
    assert_eq!(
        (mirrored.edits[0].old_text.as_str(), mirrored.edits[0].new_text.as_str()),
        ("x", "y")
    );
    let stranger = json!({"path": "a.rs", "edits": "[{\"path\">x", "newText": "y"});
    assert!(normalize(&stranger).is_err(), "only the search and replacement keys are read");
}

/// Copied from the claude-sonnet-5-5 and claude-haiku-5-5 runs that confirmed the fix above:
/// the opener in the model's own tag form, and the whole array kept in the string with one or
/// both separators garbled and a raw line break in it.
#[test]
fn the_rarer_forms_of_the_same_garble_are_read_too() {
    let tagged = json!({
        "path": "django/contrib/admindocs/utils.py",
        "edits": "\n<parameter name=\"oldText\">    indent = min(len(line)",
        "newText": "    indent = min(len(line)  # fixed",
    });
    let tagged = normalize(&tagged).expect("tag form");
    assert_eq!(tagged.edits[0].old_text, "    indent = min(len(line)");

    let kept = json!({
        "path": "django/db/models/query.py",
        "edits": "[{\"oldText\">        batch_size = 1\",\"newText\":\"        a = 1\\n        b = 2\"}]",
    });
    let kept = normalize(&kept).expect("one separator garbled");
    assert_eq!(kept.edits[0].old_text, "        batch_size = 1");
    assert_eq!(kept.edits[0].new_text, "        a = 1\n        b = 2");

    let both = json!({
        "path": "django/contrib/auth/forms.py",
        "edits": "[{\"oldText\">        x = 254\n\",\"newText\">        y = 254\n\"}]",
    });
    let both = normalize(&both).expect("both separators garbled, a raw line break inside");
    assert_eq!(both.edits[0].old_text, "        x = 254\n");
    assert_eq!(both.edits[0].new_text, "        y = 254\n");

    // A string that is garbled some other way is still refused
    let other = json!({"path": "a.rs", "edits": "[{\"oldText\": \"x\", \"newText"});
    assert!(normalize(&other).is_err());
}

#[test]
fn a_non_array_edits_value_names_the_type_that_arrived() {
    for (value, found) in
        [(json!(7), "a number"), (json!(true), "a boolean"), (json!("not json at all"), "a string")]
    {
        let args = json!({"path": "a.rs", "edits": value, "oldText": "x", "newText": "y"});
        let error = normalize(&args).expect_err("unusable");
        assert!(
            matches!(&error, ArgsError::UnusableEdits { found: got, .. } if *got == found),
            "{value} should report {found}: {error:?}"
        );
    }
}

/// A long or multi-line value is quoted as one bounded line — an error is not a diff.
#[test]
fn the_excerpt_is_one_bounded_line() {
    let long = format!("[{{\"oldText\">{}\nsecond line", "x".repeat(200));
    let args = json!({"path": "a.rs", "edits": long});
    let message = normalize(&args).expect_err("unusable").message();
    let quoted = message.lines().find(|line| line.trim_start().starts_with("[{")).unwrap();
    assert!(quoted.chars().count() <= 84, "bounded: {quoted}");
    assert!(quoted.ends_with('\u{2026}'), "elided: {quoted}");
    assert!(!message.contains("second line"), "one line only: {message}");
}

/// `edits` present but carrying nothing is empty, not missing — the distinction the old
/// message got wrong by listing the key it said had not arrived.
#[test]
fn an_edits_key_that_carries_nothing_is_reported_as_empty_not_missing() {
    for value in [json!([]), json!(null), json!("   ")] {
        let args = json!({"path": "a.rs", "edits": value});
        assert!(matches!(normalize(&args), Err(ArgsError::EmptyEdits)), "{value}");
    }
    // And a blank one does not block the single edit spelled beside it, whichever way it is
    // blank: `[]` used to return early and lose the pair that `null` kept.
    for blank in [json!(null), json!([]), json!("   ")] {
        let beside = json!({"path": "a.rs", "edits": blank, "oldText": "x", "newText": "y"});
        assert_eq!(
            normalize(&beside).expect("the top-level pair still reads").edits.len(),
            1,
            "{blank}"
        );
    }
}

/// A pair beside a *usable* array is one more edit, not one to drop on the floor.
///
/// Ignoring it applied fewer edits than were asked for and reported success, which reads to
/// the caller as "done" — the one outcome worse than a refusal. Two edits over the same text
/// is a different problem, and `super::plan`'s overlap check is what refuses that.
#[test]
fn a_top_level_pair_beside_an_array_is_appended_rather_than_dropped() {
    let request = normalize(&json!({
        "path": "a.rs",
        "edits": [{"oldText": "a", "newText": "1"}],
        "oldText": "b",
        "newText": "2",
    }))
    .expect("normalizes");
    assert_eq!(request.edits.len(), 2, "both edits survive: {:?}", request.edits);
    assert_eq!(request.edits[1].old_text, "b");
    assert_eq!(request.edits[1].new_text, "2");
}

/// A top-level single edit is not `edit[0]`, and saying so is what stops the key-order hunt.
#[test]
fn a_top_level_single_edit_is_not_reported_as_an_indexed_one() {
    let message =
        normalize(&json!({"path": "a.rs", "oldText": "x"})).expect_err("no newText").message();
    assert!(message.contains("top level"), "{message}");
    assert!(!message.contains("edit[0]"), "{message}");

    let message = normalize(&json!({"path": "a.rs", "edits": [{"oldText": "x"}]}))
        .expect_err("no newText")
        .message();
    assert!(message.contains("edit[0]"), "an array entry keeps its index: {message}");
}

/// An error has to be correctable from its own text: what arrived, and what was expected.
#[test]
fn errors_name_the_keys_received_and_the_shape_expected() {
    let message = normalize(&json!({"filename": "a.rs"})).expect_err("no path").message();
    assert!(message.contains("filename"), "names what arrived: {message}");
    assert!(message.contains("`path`"), "and what to use: {message}");
    assert!(message.contains("\"oldText\""), "and the whole shape: {message}");

    let message =
        normalize(&json!({"path": "a.rs", "edits": []})).expect_err("empty edits").message();
    assert!(message.contains("empty"));
}

/// Every refusal has to demonstrate the spelling the schema asks for.
///
/// The error text is the last thing the model reads before it writes the retry, and it is
/// measurably what re-primes the format. An error still saying `old_text` while the schema
/// says `oldText` would hand back the spelling the rename exists to stop, on the one turn the
/// model is most likely to copy verbatim. So no message may name a retired alias at all.
#[test]
fn no_error_message_names_a_retired_alias() {
    let refusals = [
        normalize(&json!({"filename": "a.rs"})),
        normalize(&json!({"path": "a.rs", "patch": "…"})),
        normalize(&json!({"path": "a.rs", "edits": []})),
        normalize(&json!({"path": "a.rs", "edits": "[{\"newText\">x"})),
        normalize(&json!({"path": "a.rs", "edits": [{"oldText": "x"}]})),
        normalize(&json!({"path": "a.rs", "oldText": "x"})),
    ];
    let retired = ["old_text", "new_text", "replace_all", "old_string", "new_string"];
    for refusal in refusals {
        let message = refusal.expect_err("a refusal").message();
        for alias in retired {
            assert!(
                !message.contains(alias),
                "a refusal offered `{alias}` back to the model:\n{message}"
            );
        }
        assert!(message.contains("oldText"), "and every one shows the shape:\n{message}");
    }
}
