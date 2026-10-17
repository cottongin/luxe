use super::*;

#[test]
fn tokenize_splits_camel_snake_and_kebab() {
    assert_eq!(tokenize("computeTotal"), vec!["compute", "total"]);
    assert_eq!(tokenize("compute_total"), vec!["compute", "total"]);
    assert_eq!(tokenize("HTTPServer"), vec!["httpserver"]);
    assert_eq!(tokenize("read_symbol2"), vec!["read", "symbol2"]);
}

#[test]
fn build_and_search_ranks_exact_prefix_then_bm25() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn compute_total() {}\npub fn compute_subtotal() {}\npub struct Total;\n",
    )
    .unwrap();
    // Ignored dir must be skipped.
    std::fs::create_dir_all(dir.join("target/x")).unwrap();
    std::fs::write(dir.join("target/x/gen.rs"), "pub fn compute_total() {}\n").unwrap();

    let index = SymbolIndex::build(&dir);
    assert_eq!(index.len(), 3, "target/ must be ignored");

    // Exact "Total" wins over token matches.
    let hits = index.search("Total", 10);
    assert_eq!(hits[0].name, "Total");

    // "compute total" (two tokens) matches both compute_* via BM25.
    let names: Vec<&str> =
        index.search("compute total", 10).iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"compute_total"));
    assert!(names.contains(&"compute_subtotal"));

    // Prefix beats a mid-name token match.
    let hits = index.search("compute", 10);
    assert!(hits[0].name.starts_with("compute"));
}

#[test]
fn prior_reorders_within_a_tier() {
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "pub fn compute_total() {}\npub fn compute_subtotal() {}\n",
    )
    .unwrap();
    let index = SymbolIndex::build(&dir);

    // Both are prefix-tier matches for "compute"; a prior favoring subtotal lifts it to the top,
    // while a neutral prior leaves the baseline (name-ordered) ranking untouched.
    let boosted: Vec<&str> =
        index
            .search_with_prior("compute", 10, |sym| {
                if sym.name == "compute_subtotal" {
                    3.0
                } else {
                    1.0
                }
            })
            .iter()
            .map(|sym| sym.name.as_str())
            .collect();
    assert_eq!(boosted.first(), Some(&"compute_subtotal"));

    let neutral: Vec<&str> =
        index.search_with_prior("compute", 10, |_| 1.0).iter().map(|s| s.name.as_str()).collect();
    let plain: Vec<&str> = index.search("compute", 10).iter().map(|s| s.name.as_str()).collect();
    assert_eq!(neutral, plain, "a neutral prior is identical to plain search");
}

/// A symbol written after the first search is found on the next one, once the file-changed
/// hook has had its say, and a search from another root reads that root.
///
/// The break this catches: the index was built once and served for the session, so what the
/// agent wrote was "No symbols matching", and a new project answered from the old tree.
#[tokio::test]
async fn the_index_follows_new_symbols_and_other_roots() {
    let (one, two) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(one.path().join("lib.rs"), "pub fn first() {}\n").unwrap();
    std::fs::write(two.path().join("lib.rs"), "pub fn elsewhere() {}\n").unwrap();
    let cache = SymbolIndexCache::default();
    assert_eq!(cache.get(one.path(), false).await.unwrap().search("first", 5).len(), 1);

    std::fs::write(one.path().join("more.rs"), "pub fn second() {}\n").unwrap();
    cache.invalidate().await;
    assert_eq!(cache.get(one.path(), false).await.unwrap().search("second", 5).len(), 1);

    let other = cache.get(two.path(), false).await.unwrap();
    assert_eq!(other.search("elsewhere", 5).len(), 1);
    assert!(other.search("first", 5).is_empty(), "not the previous project's symbols");
}

/// A tree with more source files than the index takes says it stopped, and keeps the first
/// ones in sorted order rather than whichever the filesystem listed.
#[test]
fn a_capped_walk_says_so_and_keeps_sorted_order() {
    let dir = tempfile::tempdir().unwrap();
    for index in 0..=MAX_FILES {
        std::fs::write(dir.path().join(format!("f{index:05}.rs")), "").unwrap();
    }
    let SourceFiles { files, capped } = source_files(dir.path());
    assert!(capped);
    assert_eq!(files.len(), MAX_FILES);
    assert!(files.windows(2).all(|pair| pair[0] < pair[1]), "sorted");
    assert!(files[0].ends_with("f00000.rs"));
    assert!(files.last().unwrap().ends_with(format!("f{:05}.rs", MAX_FILES - 1)), "the first ones");
    assert!(
        SymbolIndex::build(dir.path()).capped_note().contains("capped"),
        "and a search says so"
    );

    std::fs::remove_file(dir.path().join(format!("f{MAX_FILES:05}.rs"))).unwrap();
    assert!(!source_files(dir.path()).capped, "exactly at the cap is complete");
}

/// The walk honors a `.gitignore` now, as `glob` and `grep` do, and still keeps out of hidden
/// directories, which would sort first and spend the budget.
#[test]
fn ignored_files_stay_out_of_the_index() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".yarn/releases")).unwrap();
    std::fs::write(dir.path().join(".yarn/releases/yarn.js"), "function vendored() {}\n").unwrap();
    std::fs::write(dir.path().join(".gitignore"), "generated/\n").unwrap();
    std::fs::create_dir_all(dir.path().join("generated")).unwrap();
    std::fs::write(dir.path().join("generated/out.rs"), "pub fn made() {}\n").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "pub fn written() {}\n").unwrap();
    let index = SymbolIndex::build(dir.path());
    assert!(index.search("made", 5).is_empty());
    assert!(index.search("vendored", 5).is_empty(), "nothing from a hidden directory");
    assert_eq!(index.search("written", 5).len(), 1);
}
