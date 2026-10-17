use super::*;
use crate::guided_review::{parse_unified_diff, source_items, ReviewContextDraft};

fn files() -> Vec<ReviewFile> {
    parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/multi_file_semantic.diff"
    ))
    .unwrap()
    .files
}

fn context() -> ReviewContext {
    ReviewContextDraft { title: "Review".into(), ..Default::default() }.build()
}

#[test]
fn a_small_review_forms_one_batch_carrying_the_diff() {
    let files = files();
    let batches = guide_batches(&context(), &files, None);

    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].items, source_items(&files));
    // The narrator sees the code, not only path and symbol metadata.
    let addition = files
        .iter()
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .find(|line| matches!(line.kind, crate::guided_review::ReviewLineKind::Addition))
        .expect("an added line");
    assert!(batches[0].prompt.contains(addition.text.trim()));
}

#[test]
fn batches_partition_every_item_exactly_once_at_scale() {
    // Enough distinct files that no single prompt could carry them all.
    let template = files();
    let mut wide = Vec::new();
    for index in 0..400 {
        for file in &template {
            let mut clone = file.clone();
            let path = std::path::PathBuf::from(format!("src/generated_{index}.rs"));
            clone.id = crate::guided_review::FileId::from_path_lineage(None, Some(&path));
            clone.new_path = Some(path);
            clone.old_path = None;
            for (hunk_index, hunk) in clone.hunks.iter_mut().enumerate() {
                hunk.id = crate::guided_review::HunkId::from_content(
                    &clone.id,
                    &format!("{index}-{hunk_index}"),
                    "changed",
                );
            }
            wide.push(clone);
        }
    }

    let batches = guide_batches(&context(), &wide, None);
    assert!(batches.len() > 1, "a large review needs more than one pass");
    for batch in &batches {
        assert!(
            batch.prompt.chars().count() <= MAX_GUIDE_BATCH_CHARS * 2,
            "each prompt stays bounded"
        );
    }
    let batched: Vec<_> = batches.into_iter().flat_map(|batch| batch.items).collect();
    assert_eq!(batched, source_items(&wide), "every item lands in exactly one batch, in order");
}

/// Grounding used to be prepended by the caller *after* batching, so the assembled prompt could
/// exceed the budget the batching existed to guarantee.
#[test]
fn grounding_counts_against_the_batch_budget() {
    let files = files();
    let grounding = "x".repeat(MAX_GUIDE_BATCH_CHARS - 2_000);

    let plain = guide_batches(&context(), &files, None);
    let grounded = guide_batches(&context(), &files, Some(&grounding));

    assert!(grounded.len() >= plain.len(), "a large grounding block forces smaller batches");
    for batch in &grounded {
        assert!(batch.prompt.contains(&grounding), "every prompt carries the grounding");
    }
    let batched: Vec<_> = grounded.into_iter().flat_map(|batch| batch.items).collect();
    assert_eq!(batched, source_items(&files), "still exactly once, in order");
}

/// The prompt carries review context and diff text, so a secret in either must not leave here.
#[test]
fn batch_prompts_redact_known_secret_patterns() {
    let files = files();
    let secret = "sk-abcdefghijklmnopqrstuvwxyz123456";
    let context = crate::guided_review::ReviewContext::bounded(
        "Review API key",
        Some(format!("Do not expose {secret}")),
        None,
        None::<String>,
        None::<String>,
        None::<String>,
    );

    let batches = guide_batches(&context, &files, None);

    for batch in &batches {
        assert!(!batch.prompt.contains(secret));
    }
    assert!(batches.iter().any(|batch| batch.prompt.contains("[REDACTED_API_KEY]")));
}

/// The output cap has to grow with the batch, or a large section's reply is cut off mid-object
/// and rejected as invalid JSON — losing the guidance exactly where it was needed most.
#[test]
fn the_output_allowance_scales_with_the_batch_and_respects_the_model() {
    assert!(guide_max_tokens(1, 64_000) < guide_max_tokens(40, 64_000));
    assert_eq!(guide_max_tokens(400, 64_000), GUIDE_MAX_OUTPUT_TOKENS, "bounded at the top");
    assert_eq!(guide_max_tokens(40, 1_500), 1_500, "never more than the model permits");
    assert!(guide_max_tokens(1, 0) >= GUIDE_BASE_OUTPUT_TOKENS, "an unknown cap still works");
}

#[test]
fn reduce_orders_semantic_first_and_preserves_coverage() {
    let files = files();
    let items = source_items(&files);
    let groups = vec![
        GuideGroup {
            title: "Generated".into(),
            rationale: "Lockfile churn.".into(),
            impact: None,
            kind: ChapterKind::Generated,
            items: items[1..].to_vec(),
            notes: Vec::new(),
        },
        GuideGroup {
            title: "Core".into(),
            rationale: "The contract change.".into(),
            impact: None,
            kind: ChapterKind::Core,
            items: items[..1].to_vec(),
            notes: Vec::new(),
        },
    ];

    let chapters = reduce_groups(groups, &files).expect("coverage holds");
    assert_eq!(chapters[0].kind, ChapterKind::Core);
    assert_eq!(chapters[1].kind, ChapterKind::Generated);
    assert_eq!(crate::guided_review::validate_chapter_coverage(&files, &chapters), Ok(()));
}

#[test]
fn reduce_condenses_an_over_long_guide_without_dropping_items() {
    let files = files();
    let items = source_items(&files);
    // More single-item groups than the guide may present.
    let groups: Vec<GuideGroup> = items
        .iter()
        .cycle()
        .take(MAX_GUIDE_CHAPTERS + 4)
        .enumerate()
        .map(|(index, item)| GuideGroup {
            title: format!("Section {index}"),
            rationale: "A narrow slice.".into(),
            impact: None,
            kind: ChapterKind::Core,
            items: vec![item.clone()],
            notes: Vec::new(),
        })
        .take(items.len())
        .collect();
    let total_items: usize = groups.iter().map(|group| group.items.len()).sum();

    let chapters = reduce_groups(groups, &files).expect("coverage holds");
    assert!(chapters.len() <= MAX_GUIDE_CHAPTERS);
    assert_eq!(chapters.iter().map(|chapter| chapter.items.len()).sum::<usize>(), total_items);
}

#[test]
fn provenance_only_speaks_up_when_the_model_did_not_write_the_guide() {
    assert_eq!(GuideProvenance::Model { batches: 3 }.label(), None);
    assert_eq!(
        GuideProvenance::Structural { reason: None }.label().as_deref(),
        Some("structural guide")
    );
    assert_eq!(
        GuideProvenance::Mixed { model_batches: 2, structural_batches: 1, reason: None }
            .label()
            .as_deref(),
        Some("1 section grouped structurally")
    );
}

/// A fallback the reviewer cannot explain is a fallback nobody reports.
#[test]
fn a_fallback_names_its_reason() {
    assert_eq!(
        GuideProvenance::Structural { reason: Some(GuideFallback::Unusable) }.label().as_deref(),
        Some("structural guide · model reply unusable")
    );
    assert_eq!(
        GuideProvenance::Mixed {
            model_batches: 3,
            structural_batches: 2,
            reason: Some(GuideFallback::Truncated)
        }
        .label()
        .as_deref(),
        Some("2 sections grouped structurally · model reply cut off")
    );
}
