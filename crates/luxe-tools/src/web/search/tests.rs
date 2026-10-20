use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A backend with a result list per phrasing, which fails the phrasings it has no list for and
/// keeps what it was asked and how many searches it ran at once.
struct ByPhrasing {
    lists: HashMap<&'static str, Vec<&'static str>>,
    asked: Mutex<Vec<(String, usize)>>,
    in_turn: bool,
    running: AtomicUsize,
    most_at_once: AtomicUsize,
}

impl ByPhrasing {
    fn new(lists: &[(&'static str, &[&'static str])]) -> Self {
        let lists = lists.iter().map(|(query, urls)| (*query, urls.to_vec())).collect();
        let (running, most_at_once) = (AtomicUsize::new(0), AtomicUsize::new(0));
        Self { lists, asked: Mutex::new(Vec::new()), in_turn: false, running, most_at_once }
    }

    fn in_turn(self) -> Self {
        Self { in_turn: true, ..self }
    }

    /// Holds a search open long enough for another to start beside it, if one is going to.
    async fn overlap(&self) {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_at_once.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl SearchBackend for ByPhrasing {
    async fn search(
        &self,
        query: &str,
        n: usize,
        _options: &SearchOptions,
    ) -> Result<Vec<SearchHit>, WebError> {
        self.asked.lock().expect("no panic holds this lock").push((query.to_string(), n));
        self.overlap().await;
        let Some(urls) = self.lists.get(query) else {
            return Err(WebError::Backend(format!("no answer for {query}")));
        };
        Ok(urls.iter().take(n).map(|url| hit(url)).collect())
    }

    fn one_phrasing_at_a_time(&self) -> bool {
        self.in_turn
    }
}

fn hit(url: &str) -> SearchHit {
    SearchHit { title: url.to_string(), url: url.to_string(), snippet: String::new() }
}

fn queries(list: &[&str]) -> Vec<String> {
    list.iter().map(|query| query.to_string()).collect()
}

async fn searched(backend: &ByPhrasing, asked: &[&str], n: usize) -> Searched {
    let answer = run_search(backend, &queries(asked), n, &SearchOptions::default()).await;
    answer.expect("a phrasing answered")
}

async fn urls(backend: &ByPhrasing, asked: &[&str], n: usize) -> Vec<String> {
    searched(backend, asked, n).await.hits.into_iter().map(|hit| hit.url).collect()
}

/// Every phrasing gets its share of the results: the top of each, then the next of each.
///
/// The break this catches: the first phrasing's results filling every slot, so the other
/// phrasings were searched and then thrown away.
#[tokio::test]
async fn each_phrasing_gets_its_share_of_the_results() {
    let backend = ByPhrasing::new(&[("one", &["a1", "a2", "a3"]), ("two", &["b1", "b2", "b3"])]);
    assert_eq!(urls(&backend, &["one", "two"], 4).await, ["a1", "b1", "a2", "b2"]);
    // An odd count stops partway through a rank
    assert_eq!(urls(&backend, &["one", "two"], 3).await, ["a1", "b1", "a2"]);
}

/// Each phrasing is searched on its own and asked for the full count, so its own ranking and
/// passages come back.
///
/// The break this catches: the extra phrasings riding along on the first one's search, which
/// ranks and extracts passages against the first phrasing alone.
#[tokio::test]
async fn each_phrasing_is_searched_on_its_own() {
    let backend = ByPhrasing::new(&[("one", &["a1"]), ("two", &["b1"])]);
    urls(&backend, &["one", "two"], 5).await;
    let mut asked = backend.asked.lock().expect("no panic holds this lock").clone();
    asked.sort();
    assert_eq!(asked, [("one".to_string(), 5), ("two".to_string(), 5)]);
}

/// Phrasings run together unless the backend needs them in turn, and then never overlap.
///
/// The break this catches: a burst at a rate-limited endpoint, which turned all but one phrasing
/// away; or phrasings waiting on each other where nothing needed them to.
#[tokio::test]
async fn phrasings_wait_their_turn_only_where_the_backend_needs_it() {
    let lists: &[(&str, &[&str])] = &[("one", &["a1"]), ("two", &["b1"]), ("three", &["c1"])];
    let together = ByPhrasing::new(lists);
    urls(&together, &["one", "two", "three"], 5).await;
    assert_eq!(together.most_at_once.load(Ordering::SeqCst), 3, "all at once");
    let in_turn = ByPhrasing::new(lists).in_turn();
    assert_eq!(urls(&in_turn, &["one", "two", "three"], 5).await, ["a1", "b1", "c1"]);
    assert_eq!(in_turn.most_at_once.load(Ordering::SeqCst), 1, "one at a time");
}

/// A page two phrasings found is one result, and its slot goes to the next page.
///
/// The break this catches: the same URL listed twice, spending a result on a repeat.
#[tokio::test]
async fn a_page_found_twice_is_one_result() {
    let backend = ByPhrasing::new(&[("one", &["same", "a2"]), ("two", &["same", "b2"])]);
    assert_eq!(urls(&backend, &["one", "two"], 3).await, ["same", "a2", "b2"]);
}

/// A phrasing that fails is skipped and named while another answers; the error only fails the
/// search when none did.
///
/// The break this catches: one failed phrasing sinking a search the others answered, a failure
/// the model is never told about, or every phrasing failing and the reply reading as no results.
#[tokio::test]
async fn a_failed_phrasing_is_named_and_skipped_until_none_answer() {
    let backend = ByPhrasing::new(&[("works", &["a1"])]);
    let partial = searched(&backend, &["broken", "works"], 5).await;
    assert_eq!(partial.hits.len(), 1);
    assert_eq!(
        partial.skipped,
        ["\"broken\" wasn't searched: backend error: no answer for broken"]
    );
    let broken = queries(&["broken"]);
    let all_failed = run_search(&backend, &broken, 5, &SearchOptions::default()).await;
    let error = all_failed.expect_err("nothing answered");
    assert!(error.to_string().contains("no answer for broken"), "{error}");
}
