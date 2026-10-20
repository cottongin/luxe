//! Statusline segments — what each configurable module says, and the slab it rides on.

use super::activity::{activity_glyph, activity_text, thinking_color, thinking_label};
use super::{
    compact_number, segment, Ink, Palette, Segment, SegmentKind, DEFAULT_LEFT_MODULES,
    DEFAULT_RIGHT_MODULES, MAX_CWD_WIDTH, MAX_MODEL_WIDTH, MAX_PROJECT_WIDTH, MAX_TODO_WIDTH,
};
use crate::app::App;
use crate::glyphs::Icon;
use crate::widgets::truncate_end as truncate_to_width;
use luxe_core::config::StatuslineModule;
use luxe_core::provider::PlanUsage;
use luxe_core::usage::SessionUsage;

/// Builds the left cluster from the configured (or default) module list.
///
/// Project, branch and cwd are standard, always-visible statusline state (lualine convention): they
/// stay glanceable even when the Context Dock's identity section also shows them, and they're the
/// only place that carries them when the dock is hidden or in a narrow layout. Omit a module from
/// an explicit `statusline.left` to hide it. When the dock has replaced the chat, the cluster
/// becomes a dedicated Context footer instead.
pub(super) fn left_segments(
    app: &App,
    palette: Palette,
    context_replacement: bool,
) -> Vec<Segment> {
    if context_replacement {
        return vec![
            segment(SegmentKind::Heading, None, "CONTEXT", Ink::Neutral).emphasize(),
            segment(SegmentKind::Hint, None, "↑↓ sections · pgup/Dn scroll · esc", Ink::Neutral),
        ];
    }

    let modules = app.config.statusline.left.as_deref().unwrap_or(&DEFAULT_LEFT_MODULES);
    let mut segments = Vec::with_capacity(modules.len());
    for module in modules {
        let Some(segment) = left_segment(*module, app, palette) else {
            continue;
        };
        if segment.is_empty()
            || segments.iter().any(|existing: &Segment| existing.kind == segment.kind)
        {
            continue;
        }
        segments.push(segment);
    }
    segments
}

fn left_segment(module: StatuslineModule, app: &App, palette: Palette) -> Option<Segment> {
    match module {
        StatuslineModule::Project => {
            let project = live_project(app).filter(|project| !project.is_empty())?;
            Some(
                segment(
                    SegmentKind::Project,
                    Some((Icon::Folder.glyph(), Ink::Neutral)),
                    truncate_to_width(&project, MAX_PROJECT_WIDTH),
                    Ink::Neutral,
                )
                .emphasize(),
            )
        }
        StatuslineModule::Todo => {
            let plan = live_plan(app)?;
            Some(segment(
                SegmentKind::Todo,
                Some((Icon::Plan.glyph(), Ink::Neutral)),
                plan,
                Ink::Neutral,
            ))
        }
        StatuslineModule::Changes => changes_segment(app),
        StatuslineModule::Pr => pull_request_segment(app, palette),
        StatuslineModule::Branch => {
            let (branch, dirty) = live_git(app);
            let branch = branch.filter(|branch| !branch.is_empty())?;
            // The marker says it. Coloring the name too said the same thing twice, in a hue that
            // reads as a warning — and a dirty worktree is the normal state of working.
            let suffix = if dirty { "*" } else { "" };
            Some(segment(
                SegmentKind::Branch,
                Some((Icon::Branch.glyph(), Ink::Neutral)),
                format!("{}{suffix}", truncate_to_width(&branch, 20)),
                Ink::Neutral,
            ))
        }
        StatuslineModule::Cwd => {
            let cwd = live_cwd(app).filter(|cwd| !cwd.is_empty())?;
            Some(segment(
                SegmentKind::Cwd,
                Some((Icon::Folder.glyph(), Ink::Neutral)),
                abbreviate_path(&cwd, MAX_CWD_WIDTH),
                Ink::Neutral,
            ))
        }
        StatuslineModule::Activity => {
            let glyph = activity_glyph(app);
            Some(segment(
                SegmentKind::Activity,
                glyph.as_deref().map(|glyph| (glyph, Ink::Signal(palette.accent))),
                activity_text(app),
                Ink::Neutral,
            ))
        }
        _ => None,
    }
}

/// The branch's pull request, or nothing when it has none: a branch without one, or a machine
/// without `gh`, draws the bar with no module here at all.
fn pull_request_segment(app: &App, palette: Palette) -> Option<Segment> {
    let luxe_core::workspace::PullRequestPresence::Found(pr) =
        &app.workspace.as_ref()?.pull_request
    else {
        return None;
    };
    // The words carry the fact; the tint only draws the eye to a failing check or a request for
    // changes
    let icon_ink = if crate::pull_request::wants_attention(pr) {
        Ink::Signal(palette.warning)
    } else {
        Ink::Neutral
    };
    Some(segment(
        SegmentKind::PullRequest,
        Some((Icon::PullRequest.glyph(), icon_ink)),
        crate::pull_request::label(pr),
        Ink::Neutral,
    ))
}

/// `± 5 files · ⌥F` — how much has changed, and the key that shows it.
///
/// The count alone would be trivia: a number with nothing to do about it, which the Context Dock
/// already carries better. What earns the cells is the second half. This module appears exactly
/// where the dock does not — the bar and the rail are two forms of one surface — so without it
/// there is no way in to the change from a narrow terminal at all, and no sign that there is one.
///
/// The chord is read from the live keymap rather than typed in, so a remapped binding cannot leave
/// the bar advertising a key that does nothing; `/preview` is the fallback when it is unbound.
fn changes_segment(app: &App) -> Option<Segment> {
    let files = app.workspace.as_ref().map(|workspace| workspace.changes.total_files)?;
    if files == 0 {
        return None;
    }
    let opener = app
        .keymap
        .chords_for(luxe_core::keymap::UiAction::ToggleChanges)
        .and_then(|chords| chords.first())
        .map_or_else(|| "/preview".to_string(), ToString::to_string);
    Some(segment(
        SegmentKind::Changes,
        Some((Icon::Changes.glyph(), Ink::Neutral)),
        format!("{files} {} · {opener}", if files == 1 { "file" } else { "files" }),
        Ink::Neutral,
    ))
}

/// How far the session's checklist has got, or `None` when there is no checklist.
///
/// The ratio leads and the subject follows, which is what makes this safe to show. The counts are
/// arithmetic over the durable list; the *cursor* is a claim the model makes about itself, and a
/// model that stops maintaining it leaves the subject one item behind. Reading "the plan stands
/// here" survives that; reading "this is what it is doing right now" would not — which is why the
/// live turn is announced above the composer, by the runtime, and not here.
///
/// A session with no plan renders nothing at all rather than a zero, so the common case costs no
/// cells.
fn live_plan(app: &App) -> Option<String> {
    let todos = &app.workspace.as_ref()?.todos;
    if todos.total == 0 {
        return None;
    }
    let progress = format!("{}/{}", todos.completed, todos.total);
    Some(match todos.current.as_ref().filter(|item| !item.subject.is_empty()) {
        Some(item) => {
            let room = MAX_TODO_WIDTH.saturating_sub(progress.chars().count() + 3);
            format!("{progress} · {}", truncate_to_width(&item.subject, room))
        }
        None => progress,
    })
}

/// Prefers the workspace snapshot's project name, falling back to the startup status.
fn live_project(app: &App) -> Option<String> {
    if let Some(workspace) = &app.workspace {
        let name = workspace.project.display_name.clone();
        if !name.is_empty() {
            return Some(name);
        }
    }
    app.status.project.clone()
}

/// Prefers the refreshed workspace snapshot for Git identity, falling back to the startup status.
///
/// The snapshot re-collects after tool activity, so the dirty indicator tracks live edits; the
/// startup [`StatusContext`] is the fallback until the first snapshot arrives (or for non-Git
/// projects whose snapshot carries no branch).
fn live_git(app: &App) -> (Option<String>, bool) {
    if let Some(workspace) = &app.workspace {
        if workspace.project.git_branch.is_some() {
            return (workspace.project.git_branch.clone(), workspace.project.git_dirty);
        }
    }
    (app.status.git_branch.clone(), app.status.git_dirty)
}

/// Prefers the workspace snapshot's compact path, falling back to the startup status.
fn live_cwd(app: &App) -> Option<String> {
    if let Some(workspace) = &app.workspace {
        let path = workspace.project.display_path.clone();
        if !path.is_empty() {
            return Some(path);
        }
    }
    app.status.cwd.clone()
}

/// Abbreviates a display path to its anchor plus the last two components.
///
/// `~/projects/personal/luxe` → `~/…/personal/luxe`; paths with two or fewer components after
/// the anchor are kept whole. The result is clamped to `max_width` cells as a final guard.
pub(super) fn abbreviate_path(path: &str, max_width: usize) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    let (anchor, rest) = if let Some(rest) = trimmed.strip_prefix("~/") {
        ("~", rest)
    } else if let Some(rest) = trimmed.strip_prefix('/') {
        ("/", rest)
    } else {
        ("", trimmed)
    };
    let components: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    let abbreviated = if components.len() > 2 {
        let tail = components[components.len() - 2..].join("/");
        match anchor {
            "~" => format!("~/…/{tail}"),
            "/" => format!("/…/{tail}"),
            _ => format!("…/{tail}"),
        }
    } else {
        match anchor {
            "~" => format!("~/{}", components.join("/")),
            "/" => format!("/{}", components.join("/")),
            _ => components.join("/"),
        }
    };
    truncate_to_width(&abbreviated, max_width)
}

pub(super) fn right_segments(app: &App, palette: Palette) -> Vec<Segment> {
    let modules = app.config.statusline.right.as_deref().unwrap_or(&DEFAULT_RIGHT_MODULES);
    let mut segments = Vec::with_capacity(modules.len() + 1);
    // Ahead of the configured modules, and not one of them: a session approving its own escalations
    // has to say so whatever the reader's `statusline.right` says, and `--auto-approve` means
    // nobody may be watching when it starts. Drawn only while it is on — the bar reports what is
    // true, and "prompting, as usual" is not news.
    if app.permission_auto_approve {
        segments.push(segment(
            SegmentKind::AutoApprove,
            Some((Icon::Permission.glyph(), Ink::Signal(palette.warning))),
            "auto-approve",
            Ink::Signal(palette.warning),
        ));
    }
    for module in modules {
        let Some(segment) = right_segment(*module, app, palette) else {
            continue;
        };
        if segment.is_empty()
            || segments.iter().any(|existing: &Segment| existing.kind == segment.kind)
        {
            continue;
        }
        segments.push(segment);
    }
    segments
}

fn right_segment(module: StatuslineModule, app: &App, palette: Palette) -> Option<Segment> {
    match module {
        StatuslineModule::Cost => {
            let label = cost_text(app.status.plan_usage.as_ref(), &app.status.session)?;
            Some(segment(SegmentKind::Cost, None, label, Ink::Neutral))
        }
        StatuslineModule::Tokens
            if app.status.prompt_tokens > 0 || app.status.completion_tokens > 0 =>
        {
            // `↺N` shows cached input reused this turn (billed ~10x cheaper) — a visible signal
            // of prompt-cache efficiency; omitted when nothing was served from cache.
            let cache = if app.status.cache_read_tokens > 0 {
                format!(
                    " {}{}",
                    crate::glyphs::Mark::CacheReuse,
                    compact_number(app.status.cache_read_tokens)
                )
            } else {
                String::new()
            };
            Some(segment(
                SegmentKind::Tokens,
                None,
                format!(
                    "↑{} ↓{}{}",
                    compact_number(app.status.prompt_tokens),
                    compact_number(app.status.completion_tokens),
                    cache
                ),
                Ink::Neutral,
            ))
        }
        StatuslineModule::Context if app.status.context_window > 0 => {
            // The dock's ramp, not one of its own: below the warning mark this is body text, so a
            // window with room left says so by *not* being colored.
            let color =
                app.theme_styles.context_fill(app.status.context_pct).fg.unwrap_or(palette.text);
            let ink = Ink::Signal(color);
            Some(segment(
                SegmentKind::Context,
                Some((Icon::Context.glyph(), ink)),
                context_text(app.status.context_pct, app.status.context_window),
                ink,
            ))
        }
        StatuslineModule::Thinking if !app.status.model.is_empty() => {
            // Neutral unless per-level coloring is opted in: a level is a setting, not a warning.
            let ink = match thinking_color(app, palette) {
                Some(color) => Ink::Signal(color),
                None => Ink::Neutral,
            };
            Some(segment(
                SegmentKind::Thinking,
                Some((Icon::Thinking.glyph(), ink)),
                thinking_label(&app.status.thinking),
                ink,
            ))
        }
        // The bar's right anchor is where a reader looks to see what is answering, so it answers
        // either way. Luxe chooses no model on the reader's behalf, and a module that simply
        // vanished would leave the one session where that matters with nothing to explain a
        // refused turn.
        StatuslineModule::Model => {
            let (label, ink) = match app.status.model.is_empty() {
                true => ("no model".to_string(), Ink::Signal(palette.warning)),
                false => (truncate_to_width(&app.status.model, MAX_MODEL_WIDTH), Ink::Neutral),
            };
            Some(
                segment(SegmentKind::Model, Some((Icon::Model.glyph(), ink)), label, ink)
                    .emphasize(),
            )
        }
        _ => None,
    }
}

/// The cost slot's words. On a subscription the binding constraint is the plan's allowance, not a
/// dollar figure nobody is being charged, so that takes the slot when the provider reports it.
pub(super) fn cost_text(plan: Option<&PlanUsage>, session: &SessionUsage) -> Option<String> {
    plan.and_then(PlanUsage::compact)
        .or_else(|| session.total.cost_label())
        .or_else(|| session.total.plan_label())
}

/// The context reading: `4.8%/200k`.
pub(super) fn context_text(percent: f32, window: u32) -> String {
    format!("{percent:.1}%/{}", compact_number(u64::from(window)))
}
