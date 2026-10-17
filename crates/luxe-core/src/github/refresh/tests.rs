use super::*;
use serde_json::json;

#[test]
fn pushes_and_new_pull_requests_start_checks() {
    for (tool, arguments) in [
        ("bash", json!({"command": "git push"})),
        ("bash", json!({"command": "cargo fmt && git push -u origin topic"})),
        ("bash", json!({"command": "gh pr create --fill"})),
        ("bash", json!({"command": "gh run rerun 123 --failed"})),
        ("bash", json!({"command": "gh workflow run ci.yml"})),
        ("git", json!({"action": "pr", "args": ["create", "--title", "t"]})),
    ] {
        assert_eq!(touch(tool, &arguments), Touch::Started, "{tool} {arguments}");
    }
}

#[test]
fn reading_or_changing_a_pull_request_makes_the_row_stale() {
    for (tool, arguments) in [
        ("bash", json!({"command": "gh pr view"})),
        ("bash", json!({"command": "gh pr merge 12 --squash"})),
        ("bash", json!({"command": "gh issue close 3"})),
        ("bash", json!({"command": "gh run view 9 --log-failed | tail"})),
        ("git", json!({"action": "pr", "args": ["view"]})),
        ("git", json!({"action": "pr"})),
    ] {
        assert_eq!(touch(tool, &arguments), Touch::Changed, "{tool} {arguments}");
    }
}

#[test]
fn everything_else_is_nothing_github_knows_about() {
    for (tool, arguments) in [
        ("bash", json!({"command": "git status"})),
        ("bash", json!({"command": "echo gh pr view"})),
        ("bash", json!({})),
        ("git", json!({"action": "commit", "message": "gh pr create"})),
        ("read", json!({"path": "git push"})),
    ] {
        assert_eq!(touch(tool, &arguments), Touch::None, "{tool} {arguments}");
    }
}

const SETTLED: Duration = Duration::from_secs(DEFAULT_REFRESH_SECS);

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A schedule that last refreshed at `t0`, finding `standing` with plenty of budget.
fn after(t0: Instant, standing: Standing) -> Schedule {
    let mut schedule = Schedule::new(DEFAULT_REFRESH_SECS);
    schedule.refreshed(t0, standing, Some(4_000));
    schedule
}

/// The numbers the module doc and the guide promise.
#[test]
fn the_documented_rates_are_the_ones_used() {
    assert_eq!(
        [ACTIVE, SETTLED, IDLE, AWAY_AFTER, FRESH_FOR],
        [secs(15), secs(60), secs(300), secs(120), secs(600)]
    );
    assert_eq!(LOW_BUDGET, 5_000 / 5);
}

#[test]
fn the_first_refresh_is_due_at_once_and_zero_turns_it_off() {
    let now = Instant::now();
    assert_eq!(Schedule::new(60).next(now), Some(now));
    assert_eq!(Schedule::new(0).next(now), None);
}

#[test]
fn a_settled_open_pull_request_is_checked_at_the_settled_rate() {
    let t0 = Instant::now();
    let schedule = after(t0, Standing::Open { checks_pending: false });
    assert_eq!(schedule.next(t0), Some(t0 + SETTLED));
    assert_eq!(after(t0, Standing::NoPullRequest).next(t0), Some(t0 + SETTLED));
    assert_eq!(after(t0, Standing::Failed).next(t0), Some(t0 + SETTLED));
}

#[test]
fn pending_checks_are_watched_closely() {
    let t0 = Instant::now();
    let schedule = after(t0, Standing::Open { checks_pending: true });
    assert_eq!(schedule.next(t0), Some(t0 + ACTIVE));
}

/// A push starts checks GitHub may not report for a while, and a pull request opened in the
/// browser after it should turn up quickly, so for ten minutes the branch is watched closely
/// whatever the last answer was.
#[test]
fn a_fresh_push_is_watched_closely_for_ten_minutes() {
    let t0 = Instant::now();
    let mut schedule = after(t0, Standing::NoPullRequest);
    schedule.activity(t0);
    assert_eq!(schedule.next(t0), Some(t0 + ACTIVE));
    let later = t0 + FRESH_FOR - secs(1);
    schedule.refreshed(later, Standing::NoPullRequest, None);
    assert_eq!(schedule.next(later), Some(later + ACTIVE));
    let stale = t0 + FRESH_FOR;
    schedule.refreshed(stale, Standing::NoPullRequest, None);
    assert_eq!(schedule.next(stale), Some(stale + SETTLED));
}

#[test]
fn a_finished_pull_request_has_nothing_left_to_report() {
    let t0 = Instant::now();
    assert_eq!(after(t0, Standing::Finished).next(t0), Some(t0 + IDLE));
    // unless a push says something is starting again
    let mut schedule = after(t0, Standing::Finished);
    schedule.activity(t0);
    assert_eq!(schedule.next(t0), Some(t0 + ACTIVE));
}

#[test]
fn nobody_watching_for_two_minutes_slows_everything_down() {
    let t0 = Instant::now();
    let mut schedule = after(t0, Standing::Open { checks_pending: true });
    schedule.focus(t0, false);
    let soon = t0 + AWAY_AFTER - secs(1);
    assert_eq!(schedule.next(soon), Some(t0 + ACTIVE), "not away yet");
    let away = t0 + AWAY_AFTER;
    assert_eq!(schedule.next(away), Some(t0 + IDLE));
    // a second focus-out does not restart the clock, and coming back resets it
    schedule.focus(away, false);
    assert_eq!(schedule.next(away), Some(t0 + IDLE));
    schedule.focus(away, true);
    assert_eq!(schedule.next(away), Some(t0 + ACTIVE));
}

#[test]
fn a_low_budget_stops_the_hurry_but_not_the_settled_rate() {
    let t0 = Instant::now();
    let mut schedule = Schedule::new(DEFAULT_REFRESH_SECS);
    schedule.refreshed(t0, Standing::Open { checks_pending: true }, Some(LOW_BUDGET - 1));
    assert_eq!(schedule.next(t0), Some(t0 + SETTLED));
    schedule.refreshed(t0, Standing::Open { checks_pending: true }, Some(LOW_BUDGET));
    assert_eq!(schedule.next(t0), Some(t0 + ACTIVE));
    // an answer that did not report the budget keeps the last one known
    schedule.refreshed(t0, Standing::Open { checks_pending: true }, Some(10));
    schedule.refreshed(t0, Standing::Open { checks_pending: true }, None);
    assert_eq!(schedule.next(t0), Some(t0 + SETTLED));
}

/// A refusal for rate stops everything, event refreshes included, until the refill.
#[test]
fn a_rate_refusal_waits_for_the_refill() {
    let t0 = Instant::now();
    let refill = t0 + secs(900);
    let mut schedule = after(t0, Standing::Open { checks_pending: true });
    schedule.rate_limited(t0, refill);
    assert!(!schedule.may_ask(t0));
    assert!(!schedule.may_ask(refill - secs(1)));
    assert!(schedule.may_ask(refill));
    assert_eq!(schedule.next(t0), Some(refill));
    assert_eq!(schedule.next(refill), Some(t0 + ACTIVE), "due as soon as it may ask");
    assert!(Schedule::new(60).may_ask(t0));
}

#[test]
fn a_refill_already_past_does_not_hold_anything_up() {
    let t0 = Instant::now() + secs(1_000);
    let mut schedule = after(t0, Standing::NoPullRequest);
    schedule.rate_limited(t0, t0 - secs(10));
    assert!(schedule.may_ask(t0));
}

#[test]
fn an_answer_retrying_cannot_change_stops_the_interval() {
    let t0 = Instant::now();
    assert_eq!(after(t0, Standing::Unavailable).next(t0), None);
}

/// The setting is the settled rate, and the ceiling on every other: at 10 seconds it never
/// waits 15, and at ten minutes being away is no faster than being here.
#[test]
fn the_setting_bounds_the_active_and_idle_rates() {
    let t0 = Instant::now();
    let mut fast = Schedule::new(10);
    fast.refreshed(t0, Standing::Open { checks_pending: true }, Some(4_000));
    assert_eq!(fast.next(t0), Some(t0 + secs(10)));

    let mut slow = Schedule::new(600);
    slow.refreshed(t0, Standing::Finished, Some(4_000));
    assert_eq!(slow.next(t0), Some(t0 + secs(600)));
    slow.refreshed(t0, Standing::Open { checks_pending: true }, Some(4_000));
    assert_eq!(slow.next(t0), Some(t0 + ACTIVE));
}
