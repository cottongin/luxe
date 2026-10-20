use super::*;
use luxe_core::usage::UsageTotals;

/// `state_changed`, `plan_usage` and `session_usage` as a Copilot session sent them over the
/// bridge, so the shapes the phone forwards are the ones these words are made from.
const MODEL: &str = r#"{"provider":"github-copilot","model":"claude-haiku-4.5","display_name":"Claude Haiku 4.5 (latest)"}"#;
const PLAN: &str = r#"{"plan":"individual","primary":{"used_percent":22.3,"window_minutes":43200,"resets_in_secs":1106993}}"#;
const SESSION: &str = r#"{"total":{"calls":14,"prompt_tokens":122790,"completion_tokens":2212,"cache_read_tokens":103435,"cache_write_tokens":19237,"reasoning_tokens":888,"cost_usd":0.0,"unpriced_calls":0,"plan_usd":0.04556775,"plan_calls":14},"by_source":[]}"#;

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).expect("a literal copied from a real frame parses")
}

fn words(facts: &Facts<'_>) -> Vec<(Kind, String)> {
    segments(facts).into_iter().map(|segment| (segment.kind, segment.label)).collect()
}

fn spent(cost_usd: f64) -> SessionUsage {
    SessionUsage::single(
        luxe_core::usage::UsageSource::Agent,
        UsageTotals { calls: 1, cost_usd, ..UsageTotals::default() },
    )
}

/// The break this catches: the phone's wording or order drifting from the bar's, which says
/// `4.8%/200k`, the plan's share, `high` and the model, in that order.
#[test]
fn every_fact_reads_as_the_bar_words_it() {
    let (model, plan, session) = (parse(MODEL), parse(PLAN), parse(SESSION));
    let facts = Facts {
        model: Some(&model),
        thinking: Some(ThinkingLevel::High),
        context: Some((9613, 200_000)),
        plan: Some(&plan),
        session: Some(&session),
    };
    let expected = [
        (Kind::Context, "4.8%/200k"),
        (Kind::Cost, "22% monthly"),
        (Kind::Thinking, "high"),
        (Kind::Model, "Claude Haiku 4.5 (latest)"),
    ];
    let expected: Vec<_> = expected.into_iter().map(|(kind, label)| (kind, label.into())).collect();
    assert_eq!(words(&facts), expected);
}

/// The break this catches: the phone coloring a window at a different mark than the bar's ramp.
#[test]
fn the_context_reading_colors_on_the_bars_ramp() {
    let tone = |used| {
        let facts = Facts { context: Some((used, 1000)), ..Facts::default() };
        segments(&facts)[0].tone
    };
    assert_eq!([tone(749), tone(750), tone(900)], [Tone::Plain, Tone::Warning, Tone::Error]);
}

/// The break this catches: a dollar figure shown in the slot a plan's allowance owns, spend
/// dropped when there's no plan, plan-billed calls with no allowance reading shown as nothing
/// rather than what they were worth, or `$0.00` claimed before anything was priced.
#[test]
fn the_cost_slot_prefers_the_plan_then_the_spend_then_nothing() {
    let (plan, session, covered) = (parse::<PlanUsage>(PLAN), spent(0.44), parse(SESSION));
    let cost = |plan, session| {
        let facts = Facts { plan, session, ..Facts::default() };
        segments(&facts).into_iter().find(|segment| segment.kind == Kind::Cost).map(|s| s.label)
    };
    assert_eq!(cost(Some(&plan), Some(&session)).as_deref(), Some("22% monthly"));
    assert_eq!(cost(None, Some(&session)).as_deref(), Some("$0.44"));
    assert_eq!(cost(None, Some(&covered)).as_deref(), Some("≈$0.05"));
    assert_eq!(cost(None, None), None);
}

/// The break this catches: a session with no model left without a word, when saying so is what
/// explains a refused turn; or a thinking level shown for a model that isn't there.
#[test]
fn no_model_says_so_and_drops_the_thinking_level() {
    let facts = Facts { thinking: Some(ThinkingLevel::High), ..Facts::default() };
    let all = segments(&facts);
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!((all[0].label.as_str(), all[0].tone), ("no model", Tone::Warning));
}

/// The break this catches: an empty window (a provider that doesn't say its size) read as a
/// division by zero, which prints `NaN%` or `inf%`.
#[test]
fn a_window_of_unknown_size_shows_no_reading() {
    let facts = Facts { context: Some((10, 0)), ..Facts::default() };
    assert!(segments(&facts).iter().all(|segment| segment.kind != Kind::Context));
}
