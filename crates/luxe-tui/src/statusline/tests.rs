use super::*;
use crate::glyphs::{set_tier, GlyphTier};
use crate::widgets::Cap;
use luxe_core::{
    config::{Config, StatuslineModule},
    keymap::KeyMap,
};

/// The bar, the dock, the transcript, and the pickers must name a concept with the same icon;
/// re-hardcoding one here is how they drift apart.
#[test]
fn model_and_reasoning_icons_come_from_the_shared_catalog() {
    assert_eq!(Icon::Model.glyph(), Icon::Model.glyph());
    assert_eq!(Icon::Thinking.glyph(), Icon::Thinking.glyph());
}

/// A session with no model chosen says so where a session with one names it.
///
/// Luxe picks none on the reader's behalf, so this is a state the bar has to report. It used to
/// drop the module entirely, which left the one reader who needed an explanation — their turn
/// is about to be refused — looking at a bar with nothing on its right-hand end.
#[test]
fn the_bar_says_when_no_model_is_selected() {
    let mut app = app();
    assert!(app.status.model.is_empty(), "nothing is chosen for the reader");
    let bar = plain(&status_line(&app, 120));
    assert!(bar.contains("no model"), "{bar}");

    app.status.model = "claude-sonnet-5".into();
    let bar = plain(&status_line(&app, 120));
    assert!(bar.contains("claude-sonnet-5"), "{bar}");
    assert!(!bar.contains("no model"), "{bar}");
}

/// Walk-away mode is standing state, so the bar carries it — and keeps carrying it when the
/// terminal is too narrow for everything else.
///
/// The failure this pins is silent by construction: a session started with `--auto-approve`
/// approves every escalation for as long as it runs, and the only other word about it is a
/// conversation line that scrolls away. It is also not a configurable module, so a reader who
/// has curated `statusline.right` cannot end up without it.
#[test]
fn the_bar_reports_a_session_that_is_approving_its_own_asks() {
    let mut app = app();
    app.status.model = "claude-sonnet-5".into();
    assert!(!plain(&status_line(&app, 120)).contains("auto-approve"), "prompting is not news");

    app.permission_auto_approve = true;
    let bar = plain(&status_line(&app, 120));
    assert!(bar.contains("auto-approve"), "{bar}");
    assert_eq!(
        fragment_foreground(&status_line(&app, 120), "auto-approve"),
        app.theme_styles.warning.fg.expect("the warning style has a foreground"),
        "its color is part of what it says"
    );

    // A curated right-hand cluster does not get to leave it out.
    app.config.statusline.right = Some(vec![StatuslineModule::Model]);
    assert!(plain(&status_line(&app, 120)).contains("auto-approve"));

    // And it outlives the width: at 40 cells the model has gone and this has not.
    let narrow = plain(&status_line(&app, 40));
    assert!(narrow.contains("auto-approve"), "{narrow}");
}

fn app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    App::new(tx, KeyMap::defaults(), Config { theme: Some("nord".into()), ..Config::default() })
}

/// An app with the bar drawn the old way, for the assertions about `flat`.
fn flat_app() -> App {
    let mut app = app();
    app.config.statusline.style = Some(StatuslineStyle::Flat);
    app
}

fn plain(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// Foreground of the first span whose text contains `needle`.
fn fragment_foreground(line: &Line<'_>, needle: &str) -> Color {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .and_then(|span| span.style.fg)
        .unwrap_or_else(|| panic!("missing foreground for fragment containing {needle:?}"))
}

/// Background of the first span whose text contains `needle` — the slab it is sitting on.
fn fragment_background(line: &Line<'_>, needle: &str) -> Color {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .and_then(|span| span.style.bg)
        .unwrap_or_else(|| panic!("missing background for fragment containing {needle:?}"))
}

fn is_bold(line: &Line<'_>, needle: &str) -> bool {
    line.spans
        .iter()
        .find(|span| span.content.contains(needle))
        .map(|span| span.style.add_modifier.contains(Modifier::BOLD))
        .unwrap_or_else(|| panic!("missing span containing {needle:?}"))
}

fn text_position(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap_or_else(|| panic!("missing status segment {needle:?}"))
}

/// The column `needle` starts at, in cells — a byte offset would read a three-byte icon
/// swapped for a one-byte one as a two-column shift that never happened.
fn column_of(text: &str, needle: &str) -> usize {
    UnicodeWidthStr::width(&text[..text_position(text, needle)])
}

/// A crowded, realistic bar, for the tests that need every module populated.
fn busy_app() -> App {
    let mut app = app();
    app.status.project = Some("luxe".into());
    app.status.git_branch = Some("main".into());
    app.status.git_dirty = true;
    app.status.cwd = Some("~/work/luxe".into());
    app.status.context_window = 128_000;
    app.status.context_pct = 12.4;
    app.status.model = "claude-sonnet".into();
    app.status.thinking = ThinkingLevel::Medium;
    app.workspace = Some(planned_workspace(4, 9, Some("Render the Context Plan")));
    app
}

/// A snapshot carrying a checklist, since the plan module reads the workspace and nothing else.
fn planned_workspace(
    completed: usize,
    total: usize,
    current: Option<&str>,
) -> luxe_core::workspace::WorkspaceSnapshot {
    use luxe_core::workspace::{
        ProjectIdentity, TodoSummary, TodoSummaryItem, TodoSummaryStatus, WorkspaceScopeId,
        WorkspaceSnapshot,
    };
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("statusline-plan"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("main".into()),
            git_dirty: true,
            git_head: None,
        },
    );
    snapshot.todos = TodoSummary {
        current: current.map(|subject| TodoSummaryItem {
            id: "1".into(),
            subject: subject.to_string(),
            status: TodoSummaryStatus::InProgress,
            blocked_by: Vec::new(),
        }),
        completed,
        total,
        ..TodoSummary::default()
    };
    snapshot
}

#[test]
fn default_bar_leads_with_the_project_and_ends_with_the_model() {
    let model_icon = Icon::Model.glyph();
    let app = busy_app();

    let line = status_line(&app, 120);
    let text = plain(&line);
    // Still no *connected* powerline arrows (U+E0B0 / U+E0B2) — the slabs are detached, cut
    // with the chamfer pair instead, and nothing on the bar transitions onto a neighbor.
    assert!(!text.contains('\u{e0b0}'), "no left powerline arrow: {text:?}");
    assert!(!text.contains('\u{e0b2}'), "no right powerline arrow: {text:?}");

    assert!(text.contains(format!("{} luxe", Icon::Folder.glyph()).as_str()));
    assert!(text.contains(" main*"));
    assert!(text.contains("4/9 · Render the Context Plan"), "the plan leads with its ratio");
    // A path repeats what the project name already said, in a third of the cells.
    assert!(!text.contains("~/work/luxe"), "cwd is opt-in: {text:?}");
    assert!(text.contains("12.4%/128k"));
    assert!(text.contains(" med"));
    assert!(text.contains(format!("{model_icon} claude-sonnet").as_str()));
    assert_eq!(UnicodeWidthStr::width(text.as_str()), 120);
    assert!(text.find("12.4%/128k").unwrap() > text.find("main*").unwrap());
    assert!(text.find("luxe ").unwrap() < text.find("main*").unwrap());
}

/// The two facts worth anchoring wear the accent; everything between them recedes into the bar.
#[test]
fn the_bar_is_anchored_at_both_ends_and_recedes_toward_the_middle() {
    let app = busy_app();
    let shades = app.theme_styles.slab;
    let line = status_line(&app, 140);

    assert_eq!(fragment_background(&line, "luxe"), shades.hero, "the project anchors the left");
    assert_eq!(
        fragment_background(&line, "claude-sonnet"),
        shades.hero,
        "the model anchors the right"
    );
    assert_eq!(fragment_foreground(&line, "luxe"), shades.ink, "ink on the accent");
    assert!(is_bold(&line, "luxe") && is_bold(&line, "claude-sonnet"));

    assert_eq!(fragment_background(&line, "main*"), shades.at(Depth::Raised));
    assert_eq!(fragment_background(&line, "med"), shades.at(Depth::Raised));
}

/// The long, variable strings stay out of a pill — and so does the one module whose color is
/// its datum, because a surface behind it competes with the reading.
#[test]
fn the_plan_and_the_context_meter_ride_bare_on_the_bar() {
    let app = busy_app();
    let palette = palette(&app.theme_styles);
    let line = status_line(&app, 140);

    assert_eq!(fragment_background(&line, "Render the Context Plan"), palette.background);
    assert_eq!(fragment_foreground(&line, "Render the Context Plan"), palette.muted);
    assert!(!is_bold(&line, "Render the Context Plan"));

    assert_eq!(fragment_background(&line, "12.4%"), palette.background);
    // A bare segment inks neutral text muted, so body color here is the context ramp's own fill
    // showing through rather than the default a module gets for free.
    assert_eq!(fragment_foreground(&line, "12.4%"), palette.text, "keeps its fill color");
    assert_ne!(palette.text, palette.muted, "or the assertion above proves nothing");
}

/// The ratio is arithmetic over the durable list; the subject is the model's claim about its own
/// cursor. Leading with the ratio is what makes a plan the model stopped maintaining read as one
/// item behind rather than as a lie about what is happening right now.
#[test]
fn the_plan_leads_with_the_ratio_and_degrades_to_it() {
    let mut app = busy_app();

    let named = plain(&status_line(&app, 140));
    assert!(named.contains("4/9 · Render the Context Plan"), "{named:?}");

    // A plan with nothing marked in progress still has a ratio worth showing.
    app.workspace = Some(planned_workspace(4, 9, None));
    let ratio = plain(&status_line(&app, 140));
    assert!(ratio.contains("4/9"), "{ratio:?}");
    assert!(!ratio.contains(" · "), "no cursor, no subject: {ratio:?}");

    // A session that never made a plan spends no cells on saying so.
    app.workspace = Some(planned_workspace(0, 0, None));
    let none = plain(&status_line(&app, 140));
    assert!(!none.contains(Icon::Plan.glyph()), "no plan, no module: {none:?}");
}

/// It still renders, for anyone who wants the path back.
#[test]
fn the_cwd_module_renders_when_configured() {
    let mut app = busy_app();
    app.config.statusline.left = Some(vec![StatuslineModule::Cwd]);
    let line = status_line(&app, 140);
    assert!(plain(&line).contains("~/work/luxe"));
    assert_eq!(fragment_background(&line, "~/work/luxe"), palette(&app.theme_styles).background);
}

/// A dirty worktree is marked, not colored: the star already says it, and a warning hue said the
/// same thing again in a color that reads as a problem.
#[test]
fn a_dirty_branch_is_marked_rather_than_tinted() {
    let mut app = busy_app();
    let clean_ink = {
        app.status.git_dirty = false;
        fragment_foreground(&status_line(&app, 140), "main")
    };
    app.status.git_dirty = true;
    let line = status_line(&app, 140);
    assert!(plain(&line).contains("main*"), "the marker carries it");
    assert_eq!(fragment_foreground(&line, "main*"), clean_ink, "and the color does not");
    assert_eq!(fragment_foreground(&line, Icon::Branch.glyph()), clean_ink);
}

/// `flat` is the bar as it was: tinted text on the canvas, nothing behind it, wider gaps.
#[test]
fn the_flat_style_draws_no_surfaces() {
    let mut app = busy_app();
    app.config.statusline.style = Some(StatuslineStyle::Flat);
    let palette = palette(&app.theme_styles);

    let line = status_line(&app, 140);
    let text = plain(&line);
    assert!(!text.contains(Cap::Open.glyph()), "no caps: {text:?}");
    assert!(!text.contains(Cap::Close.glyph()), "no caps: {text:?}");
    for span in &line.spans {
        if let Some(bg) = span.style.bg {
            assert_eq!(bg, palette.background, "unexpected surface on {:?}", span.content);
        }
    }
    assert_eq!(fragment_foreground(&line, "claude-sonnet"), palette.muted);
}

/// Slabs cost cells, so the bar has to be measured in the style it is drawn in — measuring the
/// content alone would let a line overflow by four cells per module.
#[test]
fn the_bands_are_anchored_at_both_edges_where_the_flat_bar_is_not() {
    let app = busy_app();
    let slab = status_line(&app, 140);
    let flat = {
        let mut app = busy_app();
        app.config.statusline.style = Some(StatuslineStyle::Flat);
        status_line(&app, 140)
    };
    // Both fill the bar exactly; the slabs spend their extra cells on fades, not on overflow.
    assert_eq!(UnicodeWidthStr::width(plain(&slab).as_str()), 140);
    assert_eq!(UnicodeWidthStr::width(plain(&flat).as_str()), 140);
    // And the bands are anchored: the bar's two outermost cells are the accent itself, where
    // the flat bar leaves them on the canvas
    let shades = busy_app().theme_styles.slab;
    let (first, last) = (slab.spans.first().expect("cells"), slab.spans.last().expect("cells"));
    assert_eq!(first.style.bg, Some(shades.hero), "flush at the left edge: {:?}", slab.spans);
    assert_eq!(last.style.bg, Some(shades.hero), "flush at the right edge: {:?}", slab.spans);
    assert_ne!(flat.spans.first().and_then(|span| span.style.bg), Some(shades.hero));
}

/// Below the nerd tier a cap becomes a filled space, so the words land on exactly the same
/// columns and only the bevel is lost. `glyph_tiers.rs` holds the same promise over the whole
/// screen; this pins it at the source.
#[test]
fn every_tier_lays_the_bar_out_identically() {
    let app = busy_app();
    let mut widths = Vec::new();
    let mut columns = Vec::new();
    for tier in [GlyphTier::Nerd, GlyphTier::Unicode, GlyphTier::Ascii] {
        set_tier(tier);
        let text = plain(&status_line(&app, 140));
        widths.push(UnicodeWidthStr::width(text.as_str()));
        columns.push(column_of(&text, "main"));
    }
    set_tier(GlyphTier::Nerd);
    assert_eq!(widths, vec![140, 140, 140]);
    assert!(columns.windows(2).all(|pair| pair[0] == pair[1]), "the bar shifted: {columns:?}");
}

/// A patched font is for shine only: nothing below the nerd tier may need one.
#[test]
fn the_fallback_tiers_draw_no_private_use_codepoint() {
    let app = busy_app();
    for tier in [GlyphTier::Unicode, GlyphTier::Ascii] {
        set_tier(tier);
        for glyph in plain(&status_line(&app, 140)).chars() {
            let cp = u32::from(glyph);
            assert!(
                !(0xE000..=0xF8FF).contains(&cp) && !(0xF0000..=0xFFFFD).contains(&cp),
                "the bar at {tier:?} draws U+{cp:04X}"
            );
        }
    }
    set_tier(GlyphTier::Nerd);
}

#[test]
fn the_anchors_are_the_only_emphasized_segments() {
    let app = busy_app();
    let line = status_line(&app, 140);
    assert!(is_bold(&line, "claude-sonnet"), "the model anchor is bold");
    assert!(is_bold(&line, "luxe"), "so is the project anchor");
    assert!(!is_bold(&line, "main"), "the branch is not");
    assert!(!is_bold(&line, "12.4%"), "nor is the telemetry");
}

#[test]
fn git_identity_stays_visible_even_when_the_dock_shows_it() {
    use crate::workspace_layout::WorkspaceLayout;
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId, WorkspaceSnapshot};

    let mut app = app();
    app.status.git_branch = Some("feature/dock".into());
    let closed = plain(&status_line(&app, 120));
    assert!(closed.contains("feature/dock"));

    app.workspace = Some(WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("statusline-dock"),
        ProjectIdentity {
            root: "/project".into(),
            display_name: "project".into(),
            display_path: "~/project".into(),
            git_branch: Some("feature/dock".into()),
            git_dirty: false,
            git_head: None,
        },
    ));
    app.workspace_layout = WorkspaceLayout::Wide;
    let visible = plain(&status_line(&app, 120));
    assert!(visible.contains("feature/dock"), "branch stays visible with dock open: {visible}");
    assert!(visible.contains("project"), "so does the project name: {visible}");
}

/// The pull request module draws only when the branch has one, and says what is waiting in
/// words; every other answer leaves the bar as it was.
#[test]
fn the_pull_request_module_draws_only_when_there_is_one() {
    use luxe_core::github::{CheckRollup, PullRequestState, PullRequestStatus};
    use luxe_core::workspace::{
        ProjectIdentity, PullRequestPresence, WorkspaceScopeId, WorkspaceSnapshot,
    };

    let mut app = app();
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("statusline-pr"),
        ProjectIdentity {
            root: "/work/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/work/luxe".into(),
            git_branch: Some("topic".into()),
            git_dirty: false,
            git_head: None,
        },
    );
    app.workspace = Some(snapshot.clone());
    let bare = plain(&status_line(&app, 160));
    for absent in
        [PullRequestPresence::None, PullRequestPresence::Unavailable { reason: "no gh".into() }]
    {
        snapshot.pull_request = absent;
        app.workspace = Some(snapshot.clone());
        assert_eq!(plain(&status_line(&app, 160)), bare);
    }

    snapshot.pull_request = PullRequestPresence::Found(PullRequestStatus {
        number: 138,
        title: "Groundwork".into(),
        url: "u".into(),
        state: PullRequestState::Open,
        draft: false,
        review_decision: None,
        checks: CheckRollup { passed: 4, failed: 1, ..CheckRollup::default() },
        unresolved_threads: 2,
        head_oid: "a".into(),
        stack: None,
    });
    app.workspace = Some(snapshot);
    let visible = plain(&status_line(&app, 160));
    assert!(visible.contains("#138 · 1 failing · 2 threads"), "{visible}");
    let branch = visible.find("topic").expect("branch");
    let pr = visible.find("#138").expect("pull request");
    assert!(branch < pr, "the pull request follows its branch: {visible}");
    // Short of room, the pull request gives way before the branch it belongs to
    let narrow = plain(&status_line(&app, 65));
    assert!(narrow.contains("topic") && !narrow.contains("#138"), "{narrow}");
}

/// Inside a band, every change of color takes three cells, the fade into the canvas included,
/// and neighbors that share a color are split by a dot instead. The bar measures what it paints.
#[test]
fn every_change_of_color_in_a_band_takes_three_cells() {
    // Cost and tokens share a depth, thinking and the model do not: both kinds of boundary
    let mut app = busy_app();
    app.status.prompt_tokens = 1_200;
    app.status.completion_tokens = 340;
    app.status.session = luxe_core::usage::SessionUsage::single(
        luxe_core::usage::UsageSource::Agent,
        luxe_core::usage::UsageTotals { calls: 3, cost_usd: 0.42, ..Default::default() },
    );
    let line = status_line(&app, 160);
    assert_eq!(UnicodeWidthStr::width(plain(&line).as_str()), 160);
    // Laid out directly, so every cell outside a module's own span is accounted for: a gap on
    // the canvas, a dot between two modules of one color, or a ramp. A ramp is recognized by
    // where it is rather than by its colors, because a fade from one depth into the canvas
    // passes exactly through the next depth's shade on the way.
    let palette = palette(&app.theme_styles);
    let shape = Shape { palette, style: StatuslineStyle::Slab };
    let canvas = palette.background;
    let mut ramps = Vec::new();
    for (segments, side) in [
        (left_segments(&app, palette, false), Side::Left),
        (right_segments(&app, palette), Side::Right),
    ] {
        let laid = lay(&segments, side, shape);
        let cells: Vec<(String, Option<Color>)> = laid
            .spans
            .iter()
            .flat_map(|span| {
                span.content.chars().map(move |glyph| (glyph.to_string(), span.style.bg))
            })
            .collect();
        let inside = |column: usize| {
            laid.places.iter().any(|(_, start, width)| (*start..start + width).contains(&column))
        };
        let mut run = 0usize;
        for (column, (glyph, bg)) in cells.iter().enumerate() {
            if !inside(column) && *bg != Some(canvas) && glyph != "\u{b7}" {
                run += 1;
            } else if run > 0 {
                ramps.push(run);
                run = 0;
            }
        }
        if run > 0 {
            ramps.push(run);
        }
    }
    assert!(ramps.len() >= 4, "the bar has ramps: {ramps:?} in {:?}", plain(&line));
    assert!(ramps.iter().all(|run| *run == 3), "every ramp is three cells: {ramps:?}");
    assert!(plain(&line).contains("$0.42 · "), "same depth, split by a dot: {:?}", plain(&line));
}

/// The snapshot is the live source for all three identity modules; the startup status is only
/// the fallback until the first one arrives.
#[test]
fn configured_left_modules_show_the_live_snapshot_identity() {
    use crate::workspace_layout::WorkspaceLayout;
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId, WorkspaceSnapshot};

    let mut app = app();
    app.status.project = Some("stale".into());
    app.config.statusline.left = Some(vec![
        StatuslineModule::Project,
        StatuslineModule::Branch,
        StatuslineModule::Cwd,
        StatuslineModule::Activity,
    ]);
    app.workspace = Some(WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("statusline-left"),
        ProjectIdentity {
            root: "/home/dev/projects/personal/luxe".into(),
            display_name: "luxe".into(),
            display_path: "~/projects/personal/luxe".into(),
            git_branch: Some("master".into()),
            git_dirty: true,
            git_head: None,
        },
    ));
    app.workspace_layout = WorkspaceLayout::Wide;

    let text = plain(&status_line(&app, 140));
    assert!(text.contains(" master*"), "expected live dirty branch, got {text:?}");
    assert!(text.contains("~/…/personal/luxe"), "expected abbreviated cwd, got {text:?}");
    assert!(text.contains("luxe") && !text.contains("stale"), "live project name: {text:?}");
}

#[test]
fn configured_left_modules_can_drop_the_activity_module() {
    let mut app = app();
    app.status.git_branch = Some("main".into());
    app.config.statusline.left = Some(vec![StatuslineModule::Branch]);

    let text = plain(&status_line(&app, 120));
    assert!(text.contains(" main"));
    assert!(!text.contains("idle"), "activity omitted when unconfigured, got {text:?}");
}

/// `mode` was a module once, from an experiment in emulating a modal editor's statusline. A
/// config still naming it has to keep loading, and the bar has to ignore it.
#[test]
fn a_retired_module_name_renders_nothing() {
    let mut app = app();
    app.status.git_branch = Some("main".into());
    app.config.statusline.left = Some(vec![StatuslineModule::Unknown, StatuslineModule::Branch]);

    let text = plain(&status_line(&app, 120));
    assert!(text.contains(Icon::Branch.glyph()), "branch leads: {text:?}");
    assert!(!text.contains("NORMAL"), "no editor mode anywhere: {text:?}");
}

#[test]
fn abbreviate_path_collapses_the_middle_and_preserves_anchors() {
    assert_eq!(abbreviate_path("~/projects/personal/luxe", 40), "~/…/personal/luxe");
    assert_eq!(abbreviate_path("/home/dev/work/luxe", 40), "/…/work/luxe");
    assert_eq!(abbreviate_path("~/projects/luxe", 40), "~/projects/luxe");
    assert_eq!(abbreviate_path("~/luxe", 40), "~/luxe");
    assert_eq!(abbreviate_path("luxe", 40), "luxe");
    assert_eq!(abbreviate_path("", 40), "");

    let clamped = abbreviate_path("~/projects/personal/a-very-long-directory-name", 16);
    assert!(UnicodeWidthStr::width(clamped.as_str()) <= 16, "cwd clamps to width: {clamped:?}");
}

#[test]
fn right_cluster_orders_center_to_edge() {
    let model_icon = Icon::Model.glyph();
    let mut app = app();
    app.status.context_window = 128_000;
    app.status.context_pct = 42.0;
    app.status.prompt_tokens = 1_200;
    app.status.completion_tokens = 340;
    app.status.session = luxe_core::usage::SessionUsage::single(
        luxe_core::usage::UsageSource::Agent,
        luxe_core::usage::UsageTotals { calls: 3, cost_usd: 0.42, ..Default::default() },
    );
    app.status.model = "claude-sonnet".into();
    app.status.thinking = ThinkingLevel::Medium;

    let line = status_line(&app, 160);
    let text = plain(&line);
    let context = text_position(&text, "42.0%/128k");
    let cost = text_position(&text, "$0.42");
    let tokens = text_position(&text, "↑1.2k ↓340");
    let thinking = text_position(&text, " med");
    let model = text_position(&text, format!("{model_icon} claude-sonnet").as_str());
    assert!(context < cost && cost < tokens && tokens < thinking && thinking < model);

    // The bare meter leads, then the surfaces brighten to the edge, so the run reads as one
    // gradient rather than a row of slabs with a hole in it.
    let shades = app.theme_styles.slab;
    assert_eq!(fragment_background(&line, "42.0%"), palette(&app.theme_styles).background);
    assert_eq!(fragment_background(&line, "$0.42"), shades.at(Depth::Recessed));
    assert_eq!(fragment_background(&line, "↑1.2k"), shades.at(Depth::Recessed));
    assert_eq!(fragment_background(&line, "med"), shades.at(Depth::Raised));
    assert_eq!(fragment_background(&line, "claude-sonnet"), shades.hero);
}

#[test]
fn configured_modules_control_order_and_visibility() {
    let model_icon = Icon::Model.glyph();
    let mut app = app();
    app.status.context_window = 128_000;
    app.status.context_pct = 42.0;
    app.status.prompt_tokens = 1_200;
    app.status.session = luxe_core::usage::SessionUsage::single(
        luxe_core::usage::UsageSource::Agent,
        luxe_core::usage::UsageTotals { calls: 3, cost_usd: 0.42, ..Default::default() },
    );
    app.status.model = "claude-sonnet".into();
    app.status.thinking = ThinkingLevel::Medium;
    app.config.statusline.right =
        Some(vec![StatuslineModule::Model, StatuslineModule::Thinking, StatuslineModule::Context]);

    let line = status_line(&app, 120);
    let text = plain(&line);
    let model = text_position(&text, format!("{model_icon} claude-sonnet").as_str());
    let thinking = text_position(&text, " med");
    let context = text_position(&text, "42.0%/128k");
    assert!(model < thinking && thinking < context);
    assert!(!text.contains("↑1.2k"));
    assert!(!text.contains("$0.42"));
}

#[test]
fn unknown_and_duplicate_config_entries_are_ignored() {
    let model_icon = Icon::Model.glyph();
    let mut app = app();
    app.status.model = "claude-sonnet".into();
    app.config.statusline.right =
        Some(vec![StatuslineModule::Unknown, StatuslineModule::Model, StatuslineModule::Model]);

    let text = plain(&status_line(&app, 80));
    assert_eq!(text.matches(format!("{model_icon} claude-sonnet").as_str()).count(), 1);
    assert!(!text.contains(Icon::Thinking.glyph()));
}

#[test]
fn thinking_colors_are_opt_in_and_cover_every_level() {
    let mut app = app();
    app.status.model = "reasoning-model".into();
    let palette = palette(&app.theme_styles);

    app.status.thinking = ThinkingLevel::High;
    let neutral = status_line(&app, 90);
    assert_eq!(fragment_foreground(&neutral, "high"), palette.text, "neutral follows the slab");

    app.config.statusline.thinking_colors = Some(true);
    let cases = [
        (ThinkingLevel::Off, "off", palette.thinking_off),
        (ThinkingLevel::Minimal, "min", palette.thinking_minimal),
        (ThinkingLevel::Low, "low", palette.thinking_low),
        (ThinkingLevel::Medium, "med", palette.thinking_medium),
        (ThinkingLevel::High, "high", palette.thinking_high),
        (ThinkingLevel::XHigh, "xhi", palette.thinking_xhigh),
        (ThinkingLevel::Ultra, "ultra", palette.thinking_ultra),
    ];
    for (level, label, expected) in cases {
        app.status.thinking = level;
        let line = status_line(&app, 90);
        assert_eq!(fragment_foreground(&line, label), expected, "level {level:?}");
    }
}

/// A signal keeps its color on a receded slab, because the color is the information.
///
/// The boundaries are pinned, not just the middle of each band: this module and the dock had
/// drifted to different thresholds (70 against 75) and to different low colors (green against
/// body text), so the same session read as "fine" on one surface and "filling" on the other.
/// Only a case on each side of a mark can catch that.
#[test]
fn the_context_meter_keeps_its_warning_colors_on_a_slab() {
    let mut app = app();
    app.status.model = "claude-sonnet".into();
    app.status.context_window = 200_000;
    let palette = palette(&app.theme_styles);
    let styles = app.theme_styles.clone();
    let warning = styles.warning.fg.expect("the theme names a warning color");
    let error = styles.error.fg.expect("the theme names an error color");

    for (pct, expected) in [
        // Room left is not news, so it is body text rather than a color of its own.
        (12.0, palette.text),
        (74.9, palette.text),
        (75.0, warning),
        (89.9, warning),
        (90.0, error),
    ] {
        app.status.context_pct = pct;
        let line = status_line(&app, 120);
        assert_eq!(fragment_foreground(&line, "%/200k"), expected, "at {pct}%");
    }
    let success = styles.success.fg.expect("the theme names a success color");
    assert_ne!(palette.text, success, "the low state is no longer the success hue");
}

#[test]
fn context_replacement_shows_the_context_footer() {
    use crate::workspace_layout::WorkspaceLayout;
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId, WorkspaceSnapshot};

    let mut app = app();
    app.status.model = "claude-sonnet".into();
    app.workspace = Some(WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("replacement"),
        ProjectIdentity::non_git("/project".into(), "project", "~/project"),
    ));
    app.workspace_layout = WorkspaceLayout::Compact;
    app.dock.show();

    let line = status_line(&app, 78);
    let text = plain(&line);
    assert!(text.contains("CONTEXT"), "context footer leads: {text:?}");
    assert!(text.contains("pgup/Dn scroll"), "context footer explains scrolling: {text:?}");
    assert!(text.contains("esc"), "context footer explains return: {text:?}");
    // The badge is the anchor and the keys ride bare, not the other way round.
    let shades = app.theme_styles.slab;
    assert_eq!(fragment_background(&line, "CONTEXT"), shades.hero);
    assert_eq!(fragment_background(&line, "pgup/Dn"), palette(&app.theme_styles).background);
}

#[test]
fn narrow_lines_never_overflow() {
    for style in [StatuslineStyle::Slab, StatuslineStyle::Flat] {
        let mut app = busy_app();
        app.config.statusline.style = Some(style);
        // A crowded bar: every left module configured on, so the fit logic is exercised.
        app.config.statusline.left = Some(vec![
            StatuslineModule::Project,
            StatuslineModule::Branch,
            StatuslineModule::Cwd,
            StatuslineModule::Activity,
        ]);
        app.status.git_branch = Some("feature/a-long-branch-name".into());
        app.status.cwd = Some("~/projects/personal/luxe".into());
        app.status.context_window = 200_000;
        app.status.model = "a-very-long-model-identifier-that-needs-truncation".into();
        app.status.thinking = ThinkingLevel::XHigh;

        for width in [10usize, 20, 30, 40, 80] {
            let text = plain(&status_line(&app, width));
            assert!(
                UnicodeWidthStr::width(text.as_str()) <= width,
                "{style:?} at width {width}: {text:?} ({} cells)",
                UnicodeWidthStr::width(text.as_str())
            );
        }
    }
}

/// The bar has to survive a theme that names ANSI colors: there is nothing to interpolate, so
/// the ramp collapses rather than inventing four shades of a color the terminal owns.
#[test]
fn a_theme_with_no_blendable_accent_still_draws_slabs() {
    use luxe_core::theme::ColorValue;
    use std::collections::HashMap;

    let mut resolved = HashMap::new();
    resolved.insert("accent".to_string(), ColorValue::Ansi(3));
    resolved.insert("background".to_string(), ColorValue::Ansi(0));
    resolved.insert("selectedBg".to_string(), ColorValue::Ansi(8));
    let styles = ThemeStyles::from_resolved(&resolved, "ansi-only");

    let shades = styles.slab;
    assert_eq!(shades.hero, Color::Yellow, "the hero is still the accent");
    for depth in [Depth::Raised, Depth::Recessed] {
        assert_eq!(shades.at(depth), Color::DarkGray, "every step is the selection surface");
    }
}

/// The flat bar still exists for anyone who asks for it by name, and a future value falls back
/// to the shipped look rather than to nothing.
#[test]
fn the_style_config_round_trips_and_tolerates_the_unknown() {
    assert_eq!(app().config.statusline.style(), StatuslineStyle::Slab, "slab is the default");
    assert_eq!(flat_app().config.statusline.style(), StatuslineStyle::Flat);

    let mut app = app();
    app.config.statusline.style = Some(StatuslineStyle::Unknown);
    assert_eq!(app.config.statusline.style(), StatuslineStyle::Slab);
}
