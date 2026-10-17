use super::*;
use crate::guided_review::{parse_unified_diff, source_items, ReviewFile};

fn files() -> Vec<ReviewFile> {
    parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/multi_file_semantic.diff"
    ))
    .unwrap()
    .files
}

/// The failure that cost every real review its narrative: models package JSON in a fence or
/// behind a sentence, and the strict parser rejected the lot.
#[test]
fn model_json_survives_the_packaging_models_actually_use() {
    let manifest =
        r#"{"chapters":[{"title":"Core","rationale":"Why.","kind":"core","item_ids":["hunk:1"]}]}"#;

    for reply in [
        format!("```json\n{manifest}\n```"),
        format!("```\n{manifest}\n```"),
        format!("Here is the guide:\n\n{manifest}"),
        format!("{manifest}\n\nLet me know if you want it regrouped."),
        format!("Sure!\n```json\n{manifest}\n```\nThat covers every item."),
    ] {
        let parsed = GuideManifest::from_model_json(&reply)
            .unwrap_or_else(|error| panic!("{reply:?} should parse, got {error}"));
        assert_eq!(parsed.chapters[0].title, "Core");
    }
}

/// A `{` inside prose must not be mistaken for the manifest, and must not hide the real one.
#[test]
fn model_json_skips_a_leading_object_that_is_not_a_manifest() {
    let reply = r#"{"note":"thinking out loud"}
        {"chapters":[{"title":"Core","rationale":"Why.","kind":"core","item_ids":["hunk:1"]}]}"#;
    let parsed = GuideManifest::from_model_json(reply).expect("the manifest is found");
    assert_eq!(parsed.chapters.len(), 1);
}

/// Braces inside a rationale cannot terminate the object early.
#[test]
fn model_json_tolerates_braces_inside_strings() {
    let reply = r#"```json
        {"chapters":[{"title":"Core","rationale":"Replaces `{ }` with a struct literal.","kind":"core","item_ids":["hunk:1"]}]}
        ```"#;
    let parsed = GuideManifest::from_model_json(reply).expect("braces in strings are content");
    assert!(parsed.chapters[0].rationale.contains("{ }"));
}

/// The two ways a reply carries no manifest are reported apart, because only one of them is
/// fixable by asking for fewer tokens' worth of guide.
#[test]
fn model_json_separates_a_prose_reply_from_a_truncated_one() {
    assert!(matches!(
        GuideManifest::from_model_json("I cannot group these items.").unwrap_err(),
        GuideManifestError::NoJson
    ));
    assert!(matches!(
        GuideManifest::from_model_json(r#"{"chapters":[{"title":"Cor"#).unwrap_err(),
        GuideManifestError::Truncated
    ));
}

#[test]
fn model_json_rejects_unknown_fields() {
    let error =
        GuideManifest::from_model_json(r#"{"chapters":[],"invented_diff":"nope"}"#).unwrap_err();
    assert!(matches!(error, GuideManifestError::InvalidJson { .. }));
}

#[test]
fn manifest_filters_controls_and_preserves_exact_source_coverage() {
    let files = files();
    let items = source_items(&files);
    let manifest = GuideManifest {
        chapters: vec![GuideManifestChapter {
            title: "Core\u{0007} change".into(),
            rationale: "Groups captured\n source items.".into(),
            impact: None,
            kind: GuideManifestKind::Core,
            item_ids: items.iter().map(|item| item_key(item).into()).collect(),
            item_notes: Vec::new(),
        }],
        brief: None,
    };

    let groups = manifest.into_groups(&items).unwrap();
    assert_eq!(groups[0].title, "Core change");
    assert_eq!(groups[0].rationale, "Groups captured source items.");
    assert_eq!(groups[0].items, items);
}

/// Prose is advisory, so it is clipped; an empty field still means the section says nothing.
#[test]
fn over_long_prose_is_clipped_rather_than_thrown_away() {
    let files = files();
    let items = source_items(&files);
    let manifest = |rationale: String| GuideManifest {
        chapters: vec![GuideManifestChapter {
            title: "Core".into(),
            rationale,
            impact: None,
            kind: GuideManifestKind::Core,
            item_ids: items.iter().map(|item| item_key(item).into()).collect(),
            item_notes: Vec::new(),
        }],
        brief: None,
    };

    let groups = manifest("w".repeat(MAX_GUIDE_TEXT_CHARS + 200)).into_groups(&items).unwrap();
    assert_eq!(groups[0].rationale.chars().count(), MAX_GUIDE_TEXT_CHARS);
    assert!(groups[0].rationale.ends_with('…'));

    assert_eq!(
        manifest("   ".into()).into_groups(&items),
        Err(GuideManifestError::InvalidText { field: "rationale" })
    );
}

/// Kind is display priority. A model reaching for a neighboring word (`context`, `docs`) must
/// not cost the section its explanation.
#[test]
fn an_invented_kind_presents_as_supporting_work() {
    let files = files();
    let items = source_items(&files);
    let reply = serde_json::json!({
        "chapters": [{
            "title": "Context",
            "rationale": "Groups the surrounding change.",
            "kind": "context",
            "item_ids": items.iter().map(item_key).collect::<Vec<_>>(),
        }],
    })
    .to_string();

    let groups = GuideManifest::from_model_json(&reply).unwrap().into_groups(&items).unwrap();

    assert_eq!(groups[0].kind, ChapterKind::Supporting);
}

/// Sections arriving out of semantic order are *reordered* downstream, never rejected.
#[test]
fn sections_out_of_semantic_order_are_accepted_and_reordered() {
    let files = files();
    let items = source_items(&files);
    let manifest = GuideManifest {
        chapters: vec![
            GuideManifestChapter {
                title: "Supporting change".into(),
                rationale: "Groups supporting source items.".into(),
                impact: None,
                kind: GuideManifestKind::Supporting,
                item_ids: items[1..].iter().map(|item| item_key(item).into()).collect(),
                item_notes: Vec::new(),
            },
            GuideManifestChapter {
                title: "Core change".into(),
                rationale: "Arrives second.".into(),
                impact: None,
                kind: GuideManifestKind::Core,
                item_ids: vec![item_key(&items[0]).into()],
                item_notes: Vec::new(),
            },
        ],
        brief: None,
    };

    let groups = manifest.into_groups(&items).expect("order is not a rejection reason");
    let chapters = super::super::reduce_groups(groups, &files).expect("coverage holds");

    assert_eq!(chapters[0].kind, ChapterKind::Core, "reduce puts core first");
}

#[test]
fn manifest_rejects_chapter_counts_above_the_bounded_limit() {
    let chapter = GuideManifestChapter {
        title: "Core".into(),
        rationale: "Groups captured source items.".into(),
        impact: None,
        kind: GuideManifestKind::Core,
        item_ids: Vec::new(),
        item_notes: Vec::new(),
    };
    let manifest =
        GuideManifest { chapters: vec![chapter; MAX_GUIDE_BATCH_CHAPTERS + 1], brief: None };

    assert_eq!(manifest.into_groups(&[]), Err(GuideManifestError::TooManyChapters));
}

#[test]
fn manifest_rejects_unknown_and_duplicate_items_as_a_whole() {
    let files = files();
    let items = source_items(&files);
    let item = item_key(&items[0]).to_owned();
    let chapter = GuideManifestChapter {
        title: "Core".into(),
        rationale: "Groups one item.".into(),
        impact: None,
        kind: GuideManifestKind::Core,
        item_ids: vec![item.clone(), "hunk:not-captured".into()],
        item_notes: Vec::new(),
    };
    assert_eq!(
        GuideManifest { chapters: vec![chapter], brief: None }.into_groups(&items),
        Err(GuideManifestError::UnknownItem)
    );

    let duplicate = GuideManifestChapter {
        title: "Core".into(),
        rationale: "Repeats one item.".into(),
        impact: None,
        kind: GuideManifestKind::Core,
        item_ids: vec![item.clone(), item.clone()],
        item_notes: Vec::new(),
    };
    assert!(matches!(
        GuideManifest { chapters: vec![duplicate], brief: None }.into_groups(&items),
        Err(GuideManifestError::Coverage(ChapterCoverageError::DuplicateItem { .. }))
    ));

    let incomplete = GuideManifestChapter {
        title: "Core".into(),
        rationale: "Omits captured source items.".into(),
        impact: None,
        kind: GuideManifestKind::Core,
        item_ids: vec![item],
        item_notes: Vec::new(),
    };
    assert!(matches!(
        GuideManifest { chapters: vec![incomplete], brief: None }.into_groups(&items),
        Err(GuideManifestError::Coverage(ChapterCoverageError::MissingItems { .. }))
    ));
}

#[test]
fn model_output_rejects_missing_duplicate_and_fabricated_items() {
    let files = files();
    let items =
        source_items(&files).iter().map(|item| item_key(item).to_owned()).collect::<Vec<_>>();
    let model_output = |item_ids: Vec<String>| {
        serde_json::json!({
            "chapters": [{
                "title": "Core",
                "rationale": "Groups captured source items.",
                "impact": null,
                "kind": "core",
                "item_ids": item_ids,
            }],
        })
        .to_string()
    };

    let allowed = source_items(&files);
    let missing = GuideManifest::from_model_json(&model_output(vec![items[0].clone()])).unwrap();
    assert!(matches!(
        missing.into_groups(&allowed),
        Err(GuideManifestError::Coverage(ChapterCoverageError::MissingItems { .. }))
    ));

    let mut duplicate_items = items.clone();
    duplicate_items.push(items[0].clone());
    let duplicate = GuideManifest::from_model_json(&model_output(duplicate_items)).unwrap();
    assert!(matches!(
        duplicate.into_groups(&allowed),
        Err(GuideManifestError::Coverage(ChapterCoverageError::DuplicateItem { .. }))
    ));

    let mut fabricated_items = items;
    fabricated_items[0] = "hunk:not-captured".into();
    let fabricated = GuideManifest::from_model_json(&model_output(fabricated_items)).unwrap();
    assert_eq!(fabricated.into_groups(&allowed), Err(GuideManifestError::UnknownItem));
}

#[test]
fn manifest_rejects_control_characters_in_item_ids() {
    let files = files();
    let items = source_items(&files);
    let item = item_key(&items[0]).to_owned();
    let manifest = GuideManifest {
        chapters: vec![GuideManifestChapter {
            title: "Core".into(),
            rationale: "Attempts to smuggle a malformed source identifier.".into(),
            impact: None,
            kind: GuideManifestKind::Core,
            item_ids: vec![format!("{item}\u{0007}")],
            item_notes: Vec::new(),
        }],
        brief: None,
    };

    assert_eq!(manifest.into_groups(&items), Err(GuideManifestError::InvalidItemId));
}

/// The schema stays closed: a field we did not ask for means the reply is not a manifest.
#[test]
fn manifest_schema_stays_strict() {
    let invalid_schema = r#"{
            "chapters": [],
            "source": "model-invented"
        }"#;
    assert!(serde_json::from_str::<GuideManifest>(invalid_schema).is_err());
}
