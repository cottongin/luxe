//! Rail — the telemetry rows, their columns, and what the session says about itself.

use super::*;

/// A row of the default rail, which is what every test app draws.
pub(super) const DEFAULT_ROW: usize = CONTEXT_DOCK_WIDTH as usize - 2;

pub(super) fn app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    App::new(tx, KeyMap::defaults(), Config { theme: Some("nord".into()), ..Config::default() })
}

pub(super) fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// The rail carries walk-away mode too, because it is what remains when the bar is hidden.
///
/// Telemetry relocates here exactly then, so without this row a reader who turned the
/// statusline off would have no standing word that every escalation is being approved — and
/// turning a bar off is not consent to that.
#[test]
fn the_rail_reports_a_session_that_is_approving_its_own_asks() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    let quiet: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    assert!(
        !quiet.iter().any(|row| row.contains("auto-approve")),
        "prompting is not news: {quiet:?}"
    );

    app.permission_auto_approve = true;
    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let row = rendered
        .iter()
        .find(|row| row.starts_with("approvals"))
        .unwrap_or_else(|| panic!("no approvals row in {rendered:?}"));
    assert!(row.contains("auto-approve"), "{row}");
}

/// The surface behind the span carrying `needle` — the slab the badge is cut from.
pub(super) fn surface_behind(line: &Line<'_>, needle: &str) -> Color {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .and_then(|span| span.style.bg)
        .unwrap_or_else(|| panic!("no surface behind {needle:?}"))
}

/// The bar's rule stood on end: the accent undiluted at both anchors, receding between them.
///
/// Pinned by section rather than by color literal, so a theme cannot break it and a section
/// moving to a new depth has to say so here — which is the point, because the rail's colors are
/// only legible as a ramp while the assignment stays deliberate.
#[test]
fn the_rail_is_anchored_at_both_ends_and_recedes_between_them() {
    let app = app();
    let shades = app.theme_styles.slab;
    let surface =
        |section: Section| surface_behind(&section_heading_line(section, &app), section.label());

    for anchor in [Section::Project, Section::Model] {
        assert_eq!(surface(anchor), shades.hero, "{anchor:?} anchors the rail");
    }
    for live in [Section::Plan, Section::Files] {
        assert_eq!(surface(live), shades.at(Depth::Raised), "{live:?} is one step back");
    }
    for ambient in [Section::Elsewhere, Section::Session] {
        assert_eq!(surface(ambient), shades.at(Depth::Recessed), "{ambient:?} is two");
    }
    // Or every assertion above passes on a theme with no ramp at all.
    assert_ne!(shades.hero, shades.at(Depth::Raised));
    assert_ne!(shades.at(Depth::Raised), shades.at(Depth::Recessed));
}

/// A badge opens on the column its body rows start at, so a section reads as one block — which
/// puts its label one cap and one pad further in, at every glyph tier, because a cap keeps its
/// cell when the font cannot draw it.
#[test]
fn a_badge_opens_on_the_column_its_body_starts_at() {
    let app = app();
    for tier in [GlyphTier::Nerd, GlyphTier::Unicode, GlyphTier::Ascii] {
        set_tier(tier);
        let heading = text(&section_heading_line(Section::Files, &app));
        let at = heading.find("FILES").expect("the label is on the row");
        assert_eq!(UnicodeWidthStr::width(&heading[..at]), 2, "at {tier:?}: {heading:?}");
    }
    set_tier(GlyphTier::Nerd);
    assert_eq!(text(&text_line("12 files".into(), &app)), "12 files", "body hugs the margin");
}

/// Every badge holds one word, so the run of them down the rail reads as structure; what a
/// heading summarizes follows it on the canvas, past the closing cap.
#[test]
fn a_heading_summary_rides_bare_beside_its_badge() {
    let app = app();
    let line = section_heading_with(Section::Plan, "4/9".into(), &app);
    assert!(text(&line).ends_with(" 4/9"), "{:?}", text(&line));
    assert_eq!(surface_behind(&line, "PLAN"), app.theme_styles.slab.at(Depth::Raised));
    assert_eq!(
        surface_behind(&line, "4/9"),
        app.theme_styles.tool_bg.bg.expect("the panel names its fill"),
        "the ratio sits on the panel, not on the badge"
    );
}

/// The project row wears the statusline project module's folder, the way the branch and model
/// rows under it wear their modules' icons: one fact, drawn the same on both forms of a surface.
#[test]
fn the_project_row_carries_the_statusline_folder() {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId, WorkspaceSnapshot};
    let app = app();
    let snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("dock-project"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("main".into()),
            git_head: None,
            git_dirty: false,
        },
    );
    let model = DockSectionModel::from_snapshot(&snapshot);
    let (header, ..) = region_lines(&model, &app, 40, false, DockPresentation::SideBySide);
    let project = text(&header[1]);
    assert_eq!(project, icon_label(Icon::Folder.glyph(), "luxe"), "{project:?}");
    assert!(text(&header[2]).starts_with(&icon_label(Icon::Branch.glyph(), "")), "the branch");
}

/// One look, one setting. A bar asked to go flat takes the rail with it: this is the same shape
/// on both forms of one surface, and half of it obeying would be worse than neither.
#[test]
fn a_flat_bar_flattens_the_rail_too() {
    let mut app = app();
    app.config.statusline.style = Some(StatuslineStyle::Flat);
    let heading = section_heading_line(Section::Project, &app);
    assert_eq!(text(&heading), "PROJECT", "no caps and no padding");
    assert_eq!(heading.spans[0].style.fg, app.theme_styles.accent.fg);
}

/// The rail reports the model either way, and the row that opens the picker is there either way.
///
/// The section used to be suppressed when nothing was chosen, which hid the pointer's only way
/// into the picker from the one reader who had not chosen yet.
#[test]
fn the_rail_says_when_no_model_is_selected() {
    let app = app();
    assert!(app.status.model.is_empty(), "nothing is chosen for the reader");
    let rendered: String =
        telemetry_lines(&app).iter().map(|line| format!("{}\n", text(line))).collect();
    assert!(rendered.contains("no model"), "{rendered}");

    let mut chosen = app;
    chosen.status.model = "claude-sonnet-5".into();
    let rendered: String =
        telemetry_lines(&chosen).iter().map(|line| format!("{}\n", text(line))).collect();
    assert!(rendered.contains("claude-sonnet-5"), "{rendered}");
    assert!(!rendered.contains("no model"), "{rendered}");
}

/// The review's row holds what the review had cost while its guide is being written, and
/// shows the whole figure once it lands, rather than a batch's share of it partway through.
#[test]
fn the_review_row_waits_for_its_spend_to_settle() {
    use luxe_core::usage::{SessionUsage, SourceUsage, UsageSource, UsageTotals};
    let spent = |review: f64| {
        let totals = |cost_usd| UsageTotals { calls: 1, cost_usd, ..Default::default() };
        SessionUsage {
            total: totals(review),
            by_source: vec![SourceUsage { source: UsageSource::Review, totals: totals(review) }],
        }
    };
    let review_row = |app: &App| {
        telemetry_lines(app)
            .iter()
            .map(text)
            .find(|row| row.starts_with("review"))
            .map(|row| row.trim_start_matches("review").trim().to_owned())
    };
    let mut app = app();
    app.status.session = spent(0.10);
    assert_eq!(review_row(&app).as_deref(), Some("$0.10"));

    app.status.hold_review_cost();
    app.review.queued = Some(crate::app::QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::WorkingTree,
        state: crate::app::QueuedReviewState::Writing { since: std::time::Instant::now() },
    });
    app.status.session = spent(0.17);
    assert_eq!(review_row(&app).as_deref(), Some("$0.10"), "held while the guide is written");

    app.review.queued.as_mut().expect("queued").state = crate::app::QueuedReviewState::Ready;
    app.status.session = spent(0.32);
    assert_eq!(review_row(&app).as_deref(), Some("$0.32"), "the whole figure once it lands");
}

/// Every telemetry value opens on the same column, and it clears the badge above it.
///
/// The column only holds while every row goes through it — including the widest label there is,
/// which is a cost breakdown's.
#[test]
fn every_telemetry_value_opens_on_one_column_clear_of_the_badge() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    app.status.provider = "anthropic".into();
    app.status.context_window = 1_000_000;
    app.status.context_pct = 3.0;
    app.status.session = luxe_core::usage::SessionUsage {
        total: luxe_core::usage::UsageTotals {
            calls: 12,
            prompt_tokens: 295_000,
            completion_tokens: 5_300,
            cache_read_tokens: 266_000,
            cost_usd: 0.18,
            ..Default::default()
        },
        by_source: vec![luxe_core::usage::SourceUsage {
            source: luxe_core::usage::UsageSource::Compaction,
            totals: luxe_core::usage::UsageTotals {
                calls: 2,
                cost_usd: 0.02,
                ..Default::default()
            },
        }],
    };

    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let value_at = |label: &str| -> usize {
        let row = rendered
            .iter()
            .find(|row| row.starts_with(label))
            .unwrap_or_else(|| panic!("no {label} row in {rendered:?}"));
        UnicodeWidthStr::width(&row[..row.len() - row.trim_start_matches(label).trim_start().len()])
    };
    for label in ["provider", "model", "thinking", "context", "tokens", "cost", "compaction"] {
        assert_eq!(value_at(label), TELEMETRY_LABEL_WIDTH, "{label} in {rendered:?}");
    }

    // The badge is a cap, a pad, the word and a pad, then the closing cap.
    let badge =
        UnicodeWidthStr::width(text(&section_heading_line(Section::Session, &app)).as_str());
    assert!(badge < TELEMETRY_LABEL_WIDTH, "a value would sit under the badge: {badge}");

    // And the widest row still fits the rail it is drawn in.
    let widest = rendered.iter().map(|row| UnicodeWidthStr::width(row.as_str())).max();
    assert!(
        widest.is_some_and(|width| width <= usize::from(CONTEXT_DOCK_WIDTH) - 2),
        "the rail cannot seat it: {rendered:?}"
    );
}

/// The token row keeps the two counts and gives up the footnote when the rail is tight.
///
/// It is the only value with nothing bounding it, so it is the one that used to decide how
/// wide the label column beside it could be. Shedding cache reuse at the top of the range buys
/// that column four cells for every session, at the cost of a figure only the largest sessions
/// lose — and it is a footnote to the two counts it sits beside.
#[test]
fn the_token_row_sheds_its_footnote_rather_than_the_rail() {
    let totals = |prompt, completion, cache| luxe_core::usage::UsageTotals {
        prompt_tokens: prompt,
        completion_tokens: completion,
        cache_read_tokens: cache,
        ..Default::default()
    };

    // Ordinary and large-but-real sessions keep all three.
    let room = telemetry_value_width(DEFAULT_ROW, TELEMETRY_LABEL_WIDTH);
    let ordinary = token_value(&totals(900_000, 42_000, 600_000), room);
    assert_eq!(ordinary, format!("↑900k ↓42k {}600k", crate::glyphs::Mark::CacheReuse));
    let large = token_value(&totals(12_300_000, 340_000, 9_800_000), room);
    assert!(
        large.contains(&crate::glyphs::Mark::CacheReuse.to_string()),
        "a session with room keeps its footnote: {large}"
    );

    // The default rail now seats all three even at the top of the range
    let vast = totals(999_900_000, 999_900_000, 999_900_000);
    assert!(token_value(&vast, room).width() <= room, "the widest still overruns");

    // In a tighter row the footnote goes, and the counts never do
    let tight = "↑999.9M ↓999.9M ↺999.9M".width() - 1;
    assert_eq!(token_value(&vast, tight), "↑999.9M ↓999.9M", "the two counts are the row");

    // Nothing to report is not the same as no room to report it.
    assert_eq!(token_value(&totals(900_000, 42_000, 0), room), "↑900k ↓42k");
}

/// The label column is paid for out of the value column, so the widest value still has to fit.
///
/// Not the sample figures above but the widest the formatters can produce: `compact_number`
/// tops out at six cells (`999.9M`), and the token row spends two of those plus their marks
/// and a space. A row that outgrows the rail wraps, and a wrapped telemetry row is two rows
/// where the grid promised one — under a block pinned to the bottom of the rail, so it moves
/// what the reader was looking at.
#[test]
fn every_telemetry_row_fits_the_rail() {
    let mut app = app();
    app.status.model = "a-very-long-model-identifier".into();
    app.status.provider = "a-self-hosted-gateway".into();
    app.status.context_window = 999_999_999;
    app.status.context_pct = 99.9;
    app.permission_auto_approve = true;
    app.status.session.total = luxe_core::usage::UsageTotals {
        calls: 9,
        prompt_tokens: 999_900_000,
        completion_tokens: 999_900_000,
        cache_read_tokens: 999_900_000,
        cost_usd: 98_765.43,
        ..Default::default()
    };

    let content = usize::from(CONTEXT_DOCK_WIDTH) - 2;
    for row in telemetry_lines(&app).iter().map(text) {
        assert!(
            UnicodeWidthStr::width(row.as_str()) <= content,
            "{row:?} is {} cells, and the rail seats {content}",
            UnicodeWidthStr::width(row.as_str())
        );
    }

    // A gateway-prefixed provider reads in full as a value on the default rail
    app.status.provider = "foundry-anthropic".into();
    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    assert!(
        rendered
            .iter()
            .any(|row| row.starts_with("provider") && row.ends_with("foundry-anthropic")),
        "the provider is elided: {rendered:?}"
    );

    // The label column is paid for out of a model's name, so a routine one fits whole
    let name_room = telemetry_value_width(DEFAULT_ROW, TELEMETRY_LABEL_WIDTH)
        .saturating_sub(icon_cells(Icon::Model.glyph()));
    assert!(
        name_room >= "Claude Sonnet 4.6 (EU)".width(),
        "a routine model name does not fit in {name_room} cells"
    );
}

/// The column is sized to our own labels, so it has to seat the widest of them, a cost source's.
#[test]
fn the_label_column_seats_every_label() {
    use luxe_core::usage::UsageSource;
    let sources = [UsageSource::Agent, UsageSource::Review, UsageSource::Compaction];
    let labels = sources.iter().map(|source| source.label()).chain([
        UsageSource::CacheWarm.label(),
        "provider",
        "approvals",
    ]);
    for label in labels {
        assert!(label.width() <= WIDEST_TELEMETRY_LABEL, "{label} is wider than the column");
    }
}

/// The model's mark comes from its id, so a model named in config still wears its maker's mark.
/// Once another model is reported, the stale one's id no longer speaks for it.
#[test]
fn a_named_model_is_marked_by_its_id() {
    let mut app = app();
    app.active_model_info = Some(luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("anthropic", "claude-opus-5-5"),
        display_name: Some("opus".into()),
        ..Default::default()
    });
    app.status.model = "opus".into();
    app.status.provider = "anthropic".into();
    assert_eq!(super::telemetry::model_id(&app), "claude-opus-5-5");

    app.status.model = "gpt-5".into();
    app.status.provider = "openai".into();
    assert_eq!(super::telemetry::model_id(&app), "gpt-5");
}

/// The provider is a row of its own, label left and value right like every other row, directly
/// above the model it serves, and wears its own mark.
#[test]
fn the_provider_is_its_own_row_above_the_model() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    app.status.provider = "anthropic".into();

    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let model = rendered.iter().position(|row| row.contains("claude-sonnet-5")).expect("a model");
    assert!(rendered[model].starts_with("model "), "{:?}", rendered[model]);
    assert!(
        rendered[model]
            .ends_with(&crate::glyphs::icon_label(Icon::Model.glyph(), "claude-sonnet-5")),
        "the value keeps its own mark: {:?}",
        rendered[model]
    );
    let provider = &rendered[model - 1];
    let mark = crate::widgets::provider_icon("anthropic").glyph();
    assert!(provider.starts_with("provider "), "{provider:?}");
    assert!(provider.ends_with(&crate::glyphs::icon_label(mark, "anthropic")), "{provider:?}");
}

/// A provider longer than its value column is cut at the rail's edge, and keeps the column.
#[test]
fn a_long_provider_is_cut_in_its_value_column() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    app.status.provider = "a-self-hosted-gateway-with-a-very-long-name".into();

    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let provider = rendered.iter().find(|row| row.starts_with("provider")).expect("a provider");
    let value = provider.trim_start_matches("provider").trim_start();
    assert_eq!(
        UnicodeWidthStr::width(&provider[..provider.len() - value.len()]),
        TELEMETRY_LABEL_WIDTH,
        "the value moved off its column: {provider:?}"
    );
    assert!(UnicodeWidthStr::width(provider.as_str()) <= DEFAULT_ROW, "{provider:?} overruns");
    assert!(!provider.contains("very-long-name"), "nothing was cut: {provider:?}");
}

/// A rail dragged wide gives the grid air; the default rail keeps its floor.
#[test]
fn a_wide_rail_widens_the_grid() {
    let mut app = app();
    app.status.model = "claude-opus-5".into();
    app.status.provider = "foundry-anthropic".into();
    app.status.context_window = 1_000_000;
    let value_column = |app: &App| -> Vec<usize> {
        telemetry_lines(app)
            .iter()
            .map(text)
            .filter(|row| row.starts_with("provider") || row.starts_with("context"))
            .map(|row| {
                let label = row.split_whitespace().next().unwrap_or_default().to_string();
                let value = row[label.len()..].trim_start();
                UnicodeWidthStr::width(&row[..row.len() - value.len()])
            })
            .collect()
    };
    assert_eq!(value_column(&app), [TELEMETRY_LABEL_WIDTH; 2], "the default rail is the floor");

    app.dock_rail.set(crate::workspace_layout::DOCK_MAX_WIDTH);
    let wide = value_column(&app);
    assert_eq!(wide, [WIDEST_TELEMETRY_LABEL + 4; 2], "four cells of air, one grid: {wide:?}");
}

#[test]
fn the_label_column_grows_only_where_there_is_room() {
    let widest = WIDEST_TELEMETRY_LABEL;
    assert_eq!(telemetry_label_width(40, widest), TELEMETRY_LABEL_WIDTH);
    assert_eq!(telemetry_label_width(40, 30), 16, "never past two fifths of the row");
    assert_eq!(telemetry_label_width(78, 8), TELEMETRY_LABEL_WIDTH, "short labels keep the floor");
    assert_eq!(telemetry_label_width(78, widest), widest + 4);
    assert_eq!(telemetry_label_width(78, 60), 31, "never past two fifths of the row");
}

/// Before the engine has said anything there is no provider to name, so the row says `none`
/// rather than vanishing and moving every row under it.
#[test]
fn a_model_with_no_provider_yet_keeps_its_rows() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();

    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let model = rendered.iter().position(|row| row.contains("claude-sonnet-5")).expect("a model");
    assert!(rendered[model].starts_with("model"), "{:?}", rendered[model]);
    assert!(rendered[model - 1].starts_with("provider"), "{rendered:?}");
    assert!(rendered[model - 1].ends_with("none"), "{rendered:?}");
}

/// One block for what the session consumed, so the window is not a section of its own.
///
/// Three headings over five rows spent as much rail on the badges as on the numbers. What keeps
/// the window from reading as more accounting was never the badge — it is the color, which it
/// keeps.
#[test]
fn the_window_is_read_under_session_rather_than_a_heading_of_its_own() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    app.status.context_window = 200_000;
    app.status.context_pct = 42.0;

    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    assert!(!rendered.iter().any(|row| row.contains("CONTEXT")), "{rendered:?}");
    assert!(!rendered.iter().any(|row| row.starts_with("used")), "{rendered:?}");

    let session = rendered.iter().position(|row| row.contains("SESSION")).expect("a heading");
    let window = rendered.iter().position(|row| row.starts_with("context")).expect("a window");
    assert_eq!(window, session + 1, "the window leads the block: {rendered:?}");
    assert!(rendered[window].ends_with("42% of 200k"), "{:?}", rendered[window]);
    assert!(rendered[window].contains(Icon::Context.glyph()), "{:?}", rendered[window]);
}

/// Occupancy is a figure, not a chart — the same rule `/context` already holds itself to.
///
/// A ten-cell bar between the glyph and the number drew what the number already said, and left
/// this the one row in the block whose value missed the column its neighbors share. At the
/// occupancy a session spends most of its life at it was mostly empty rail, which read as a
/// trench rather than a meter. The severity hue survives it, because that part is information.
#[test]
fn the_window_is_a_figure_rather_than_a_bar() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    app.status.context_window = 200_000;

    // The section badges are cut from Block Elements themselves, so they are dropped before
    // the scan rather than exempting their whole row — a gauge smuggled back into a heading
    // would still be caught.
    let caps = [crate::widgets::Cap::Open.glyph(), crate::widgets::Cap::Close.glyph()];
    for percent in [0.0, 12.4, 42.0, 80.0, 95.0, 100.0] {
        app.status.context_pct = percent;
        let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
        assert!(
            !rendered.iter().any(|row| row
                .chars()
                .filter(|glyph| !caps.contains(&glyph.to_string().as_str()))
                .any(|glyph| ('\u{2580}'..='\u{259f}').contains(&glyph))),
            "a block element is drawn at {percent}%: {rendered:?}"
        );
    }

    // And the figure opens on the value column, level with the rows underneath it.
    let rendered: Vec<String> = telemetry_lines(&app).iter().map(text).collect();
    let window = rendered.iter().find(|row| row.starts_with("context")).expect("a window");
    let sibling = rendered.iter().find(|row| row.starts_with("thinking")).expect("a sibling");
    let opens_at = |row: &str, label: &str| {
        UnicodeWidthStr::width(&row[..row.len() - row.trim_start_matches(label).trim_start().len()])
    };
    assert_eq!(
        opens_at(window, "context"),
        opens_at(sibling, "thinking"),
        "{window:?} against {sibling:?}"
    );
}
