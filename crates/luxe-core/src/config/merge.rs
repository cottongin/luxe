//! Merging — how a project's config layers over the user's, one block at a time.
//!
//! The project wins a key it sets, except where a block says otherwise, and each block's
//! exceptions sit in its own impl: what a cloned repository may not grant itself, may only turn
//! down, or replaces whole rather than appending to.

use super::{
    AgentConfig, CompactionConfig, Config, ConversationConfig, DiffConfig, DockConfig, GateConfig,
    GithubConfig, LspConfig, OnAsk, PlanConfig, ReviewConfig, ReviewerConfig, StatuslineConfig,
    ToolsConfig, UpdateConfig, WorktreeConfig,
};
use std::collections::HashMap;

/// A config block that knows how a project's copy layers over the user's.
///
/// Every impl takes the project's copy apart field by field, so a field added to a block does not
/// compile until someone has decided how it layers.
pub(super) trait Layered {
    fn layered(global: Self, project: Self) -> Self;
}

impl Layered for Config {
    fn layered(global: Self, project: Self) -> Self {
        let Self {
            theme,
            colors,
            show_key_hints,
            mouse,
            dashboard,
            banner_animation,
            glyphs,
            images,
            notifications,
            notification_method,
            bell,
            herdr,
            joinable,
            statusline,
            conversation,
            dock,
            diff,
            review,
            tools,
            agent,
            gate,
            plan,
            reviewer,
            compaction,
            lsp,
            update,
            github,
            worktree,
            launchers: _,
            mentions: _,
        } = project;
        Self {
            theme: theme.or(global.theme),
            colors: merge_color_overrides(global.colors, colors),
            show_key_hints: show_key_hints.or(global.show_key_hints),
            mouse: mouse.or(global.mouse),
            dashboard: dashboard.or(global.dashboard),
            banner_animation: banner_animation.or(global.banner_animation),
            glyphs: glyphs.or(global.glyphs),
            images: images.or(global.images),
            notifications: notifications.or(global.notifications),
            notification_method: notification_method.or(global.notification_method),
            bell: bell.or(global.bell),
            herdr: herdr.or(global.herdr),
            // A project can close its sessions to other front-ends, never open what the user closed
            joinable: match global.joinable {
                Some(false) => Some(false),
                _ => joinable.or(global.joinable),
            },
            statusline: Layered::layered(global.statusline, statusline),
            conversation: Layered::layered(global.conversation, conversation),
            dock: Layered::layered(global.dock, dock),
            diff: Layered::layered(global.diff, diff),
            review: Layered::layered(global.review, review),
            tools: Layered::layered(global.tools, tools),
            agent: Layered::layered(global.agent, agent),
            gate: Layered::layered(global.gate, gate),
            plan: Layered::layered(global.plan, plan),
            reviewer: Layered::layered(global.reviewer, reviewer),
            compaction: Layered::layered(global.compaction, compaction),
            lsp: Layered::layered(global.lsp, lsp),
            update: Layered::layered(global.update, update),
            github: Layered::layered(global.github, github),
            worktree: Layered::layered(global.worktree, worktree),
            // `global` only: a key that runs a program a cloned repository named is its hooks by
            // another door
            launchers: global.launchers,
            // `global` only, for the same reason: a source runs a program
            mentions: global.mentions,
        }
    }
}

impl Layered for StatuslineConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { visibility, left, right, thinking_colors, style } = project;
        Self {
            visibility: visibility.or(global.visibility),
            left: left.or(global.left),
            right: right.or(global.right),
            thinking_colors: thinking_colors.or(global.thinking_colors),
            style: style.or(global.style),
        }
    }
}

impl Layered for ConversationConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self {
            prose_max_width,
            wide_block_max_width,
            tool_details,
            alignment,
            growth,
            thinking,
            scroll_lines,
            activity_bar,
            image_max_rows,
            stream_reveal,
            tool_runs,
        } = project;
        Self {
            prose_max_width: prose_max_width.or(global.prose_max_width),
            wide_block_max_width: wide_block_max_width.or(global.wide_block_max_width),
            tool_details: tool_details.or(global.tool_details),
            alignment: alignment.or(global.alignment),
            growth: growth.or(global.growth),
            thinking: thinking.or(global.thinking),
            scroll_lines: scroll_lines.or(global.scroll_lines),
            activity_bar: activity_bar.or(global.activity_bar),
            image_max_rows: image_max_rows.or(global.image_max_rows),
            stream_reveal: stream_reveal.or(global.stream_reveal),
            tool_runs: tool_runs.or(global.tool_runs),
        }
    }
}

impl Layered for DockConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { mode, position } = project;
        Self { mode: mode.or(global.mode), position: position.or(global.position) }
    }
}

impl Layered for DiffConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { layout, word_wrap } = project;
        Self { layout: layout.or(global.layout), word_wrap: word_wrap.or(global.word_wrap) }
    }
}

impl Layered for ReviewConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { context_files, exclude } = project;
        Self {
            exclude: replaced(global.exclude, exclude),
            context_files: replaced(global.context_files, context_files),
        }
    }
}

impl Layered for ToolsConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { disabled, format_on_save, clone_repositories } = project;
        Self {
            disabled: merge_disabled(global.disabled, disabled),
            format_on_save: format_on_save.or(global.format_on_save),
            clone_repositories: clone_repositories.or(global.clone_repositories),
        }
    }
}

impl Layered for AgentConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { max_tool_iterations, remember_model, on_ask, cache_warming, default_mode } =
            project;
        let warming_ceiling = global.cache_warming();
        Self {
            max_tool_iterations: max_tool_iterations.or(global.max_tool_iterations),
            remember_model: remember_model.or(global.remember_model),
            // Tighten-only, the same rule as a project's `permissions.json`: a repository may
            // refuse what its own code is allowed to do and may not grant it. A project asking for
            // `allow` is therefore ignored rather than honored — it is the one direction that
            // would let a clone widen policy.
            on_ask: match on_ask {
                Some(OnAsk::Deny) => Some(OnAsk::Deny),
                _ => global.on_ask,
            },
            // Down-only, for the same reason: warming spends the user's money, and a cloned
            // repository may decline to but may not volunteer more of it.
            cache_warming: cache_warming
                .map(|mode| mode.min(warming_ceiling))
                .or(global.cache_warming),
            default_mode: default_mode.or(global.default_mode),
        }
    }
}

impl Layered for GateConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { commands, timeout_secs, review } = project;
        Self {
            // A trusted project's commands replace rather than append another repo's suite
            commands: replaced(global.commands, commands),
            timeout_secs: timeout_secs.or(global.timeout_secs),
            review: review.or(global.review),
        }
    }
}

impl Layered for PlanConfig {
    // Field by field, so a project that names only plan files keeps your model
    fn layered(global: Self, project: Self) -> Self {
        let Self { files, model, thinking } = project;
        Self {
            files: replaced(global.files, files),
            model: model.or(global.model),
            thinking: thinking.or(global.thinking),
        }
    }
}

impl Layered for ReviewerConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { model, thinking, tools } = project;
        Self {
            model: model.or(global.model),
            thinking: thinking.or(global.thinking),
            tools: tools.or(global.tools),
        }
    }
}

impl Layered for CompactionConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { auto, at, keep_recent, reserve, prune, summarizer } = project;
        Self {
            auto: auto.or(global.auto),
            at: at.or(global.at),
            keep_recent: keep_recent.or(global.keep_recent),
            reserve: reserve.or(global.reserve),
            prune: prune.or(global.prune),
            summarizer: summarizer.or(global.summarizer),
        }
    }
}

impl Layered for LspConfig {
    // `global` only, deliberately: the project does not get a say in whether its own binaries may
    // run. See [`LspConfig::project_servers`].
    fn layered(global: Self, project: Self) -> Self {
        let Self { project_servers: _ } = project;
        Self { project_servers: global.project_servers }
    }
}

impl Layered for UpdateConfig {
    // `global` only, same reasoning in a lower key: whether luxe reaches the network on your
    // behalf is your decision, not a decision a repository you cloned makes.
    fn layered(global: Self, project: Self) -> Self {
        let Self { check: _ } = project;
        Self { check: global.check }
    }
}

impl Layered for GithubConfig {
    // The interval is the user's alone; a project's sections are searches run only when someone
    // opens `/github`, so the project may add its own
    fn layered(global: Self, project: Self) -> Self {
        let Self { refresh_secs: _, sections } = project;
        Self { refresh_secs: global.refresh_secs, sections: sections.or(global.sections) }
    }
}

impl Layered for WorktreeConfig {
    fn layered(global: Self, project: Self) -> Self {
        let Self { always } = project;
        Self { always: always.or(global.always) }
    }
}

/// Merges per-token color overrides, project entries winning key-by-key over global ones, so a
/// project can tweak one color without dropping the user's other global overrides.
pub(super) fn merge_color_overrides(
    global: Option<HashMap<String, String>>,
    project: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    match (global, project) {
        (Some(mut merged), Some(project)) => {
            merged.extend(project);
            Some(merged)
        }
        (global, project) => project.or(global),
    }
}

/// Union of two tool deny-lists (global + project), de-duplicated and order-preserving.
pub(super) fn merge_disabled(global: Vec<String>, project: Vec<String>) -> Vec<String> {
    let mut merged = global;
    for name in project {
        if !merged.contains(&name) {
            merged.push(name);
        }
    }
    merged
}

/// A list the project replaces whole when it sets one, rather than appending to the user's.
fn replaced<T>(global: Vec<T>, project: Vec<T>) -> Vec<T> {
    if project.is_empty() {
        global
    } else {
        project
    }
}
