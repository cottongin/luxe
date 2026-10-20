//! The TUI's renderer, handed to the bridge so the phone draws text the way the terminal does.

use luxe_remote::{StatusFacts, StatusSegment, StatusTone};
use luxe_tui::markdown::html;
use luxe_tui::statusline::plain;

/// Markdown, highlighting, tool glyphs and the status line's wording from `luxe-tui`.
pub(super) struct TuiRenderer;

impl luxe_remote::Renderer for TuiRenderer {
    fn markdown(&self, text: &str, voice: luxe_remote::Voice) -> String {
        let voice = match voice {
            luxe_remote::Voice::Answer => html::Voice::Answer,
            luxe_remote::Voice::Reasoning => html::Voice::Reasoning,
        };
        html::render(text, voice)
    }

    fn tool_glyph(&self, tool: &str) -> String {
        // the app's font has the Nerd Font marks whatever this terminal can draw
        luxe_tui::widgets::tool_icon(tool).at(luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn status(&self, facts: &StatusFacts) -> Vec<StatusSegment> {
        let facts = plain::Facts {
            model: facts.model.as_ref(),
            thinking: facts.thinking,
            context: facts.context.map(|usage| (usage.used_tokens, usage.context_window)),
            plan: facts.plan.as_ref(),
            session: facts.session.as_ref(),
        };
        plain::segments(&facts).into_iter().map(status_segment).collect()
    }

    fn image_glyph(&self) -> String {
        luxe_tui::glyphs::Icon::Image.at(luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn code_line(&self, text: &str, language: Option<&str>) -> String {
        html::code_line(text, language)
    }

    fn todo_mark(&self, status: luxe_core::TodoSummaryStatus) -> String {
        luxe_tui::marks::todo(status, luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn change_mark(&self, status: luxe_core::workspace::FileChangeStatus) -> String {
        luxe_tui::marks::change(status).to_string()
    }

    fn file_mark(&self, name: &str) -> luxe_remote::FileMark {
        // the extension, as the TUI's tree reads it for a file's mark
        let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
        let icon = luxe_tui::widgets::language_icon(&extension.to_ascii_lowercase());
        let glyph = icon.at(luxe_tui::glyphs::GlyphTier::Nerd).to_string();
        luxe_remote::FileMark { glyph, hue: mark_hue(luxe_tui::widgets::language_hue_role(icon)) }
    }

    fn folder_mark(&self) -> String {
        luxe_tui::glyphs::Icon::Folder.at(luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn branch_mark(&self) -> String {
        luxe_tui::glyphs::Icon::Branch.at(luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn mode_mark(&self, mode: &str) -> String {
        luxe_tui::modes::icon(mode).at(luxe_tui::glyphs::GlyphTier::Nerd).to_string()
    }

    fn github_marks(&self) -> luxe_remote::GithubMarks {
        use luxe_tui::glyphs::{GlyphTier::Nerd, Icon, Mark};
        luxe_remote::GithubMarks {
            pull_request: Icon::PullRequest.at(Nerd).to_owned(),
            issue: Icon::Issue.at(Nerd).to_owned(),
            yes: Mark::Yes.at(Nerd).to_owned(),
            no: Mark::No.at(Nerd).to_owned(),
            waiting: Mark::Waiting.at(Nerd).to_owned(),
            elision: Mark::Elision.at(Nerd).to_owned(),
        }
    }

    fn ago(&self, at: u64, now: u64) -> String {
        luxe_tui::github_detail::ago(at, now)
    }

    fn github_readable(&self, body: &str) -> String {
        luxe_tui::github_detail::readable(body)
    }

    fn home_icon(&self, side: u32, colors: luxe_remote::TileColors) -> Option<Vec<u8>> {
        luxe_tui::widgets::tile_png(side, colors.plate, colors.ink)
            .inspect_err(|error| tracing::warn!(%error, side, "could not draw the app icon"))
            .ok()
    }
}

fn mark_hue(hue: luxe_tui::widgets::LanguageHue) -> luxe_remote::MarkHue {
    use luxe_remote::MarkHue;
    use luxe_tui::widgets::LanguageHue;
    match hue {
        LanguageHue::Warning => MarkHue::Warning,
        LanguageHue::Info => MarkHue::Info,
        LanguageHue::Success => MarkHue::Success,
        LanguageHue::Error => MarkHue::Error,
        LanguageHue::ThinkingUltra => MarkHue::ThinkingUltra,
        LanguageHue::Text => MarkHue::Text,
        LanguageHue::Muted => MarkHue::Muted,
    }
}

fn status_segment(segment: plain::Segment) -> StatusSegment {
    let kind = match segment.kind {
        plain::Kind::Context => "context",
        plain::Kind::Cost => "cost",
        plain::Kind::Thinking => "thinking",
        plain::Kind::Model => "model",
    };
    let tone = match segment.tone {
        plain::Tone::Plain => StatusTone::Plain,
        plain::Tone::Warning => StatusTone::Warning,
        plain::Tone::Error => StatusTone::Error,
    };
    StatusSegment { kind, glyph: segment.glyph.map(str::to_string), label: segment.label, tone }
}

#[cfg(test)]
mod tests {
    use luxe_remote::{Renderer, Voice};

    use super::TuiRenderer;

    /// The break this catches: the voices crossed, so reasoning loses its line breaks and an
    /// answer keeps the model's.
    #[test]
    fn each_voice_reaches_the_tui_as_itself() {
        assert_eq!(TuiRenderer.markdown("a\nb", Voice::Reasoning), "<p>a<br>b</p>");
        assert_eq!(TuiRenderer.markdown("a\nb", Voice::Answer), "<p>a b</p>");
    }

    /// The break this catches: the glyph taken at this terminal's tier, which is ASCII or none
    /// under a plain terminal, though the app's font always has the marks.
    #[test]
    fn tool_glyphs_are_the_nerd_font_marks() {
        let nerd = luxe_tui::widgets::tool_icon("bash").at(luxe_tui::glyphs::GlyphTier::Nerd);
        luxe_tui::glyphs::set_tier(luxe_tui::glyphs::GlyphTier::Ascii);
        let glyph = TuiRenderer.tool_glyph("bash");
        let image = TuiRenderer.image_glyph();
        let done = TuiRenderer.todo_mark(luxe_core::TodoSummaryStatus::Completed);
        luxe_tui::glyphs::set_tier(luxe_tui::glyphs::GlyphTier::Nerd);
        assert_eq!(glyph, nerd);
        let nerd_image = luxe_tui::glyphs::Icon::Image.at(luxe_tui::glyphs::GlyphTier::Nerd);
        assert_eq!(image, nerd_image, "an attached image's mark too");
        let nerd_done = luxe_tui::glyphs::Mark::Yes.at(luxe_tui::glyphs::GlyphTier::Nerd);
        assert_eq!(done, nerd_done, "a todo's mark too");
    }

    /// The break this catches: a diff row's code left plain, or a changed file's letter crossed,
    /// so the phone's dock and diffs stop matching the TUI's.
    #[test]
    fn diffs_and_changes_reach_the_tui() {
        let code = TuiRenderer.code_line("fn a() {}", Some("rs"));
        assert!(code.starts_with("<span class=\"t-accent-secondary\">fn</span>"), "{code}");
        let modified = luxe_core::workspace::FileChangeStatus::Modified;
        assert_eq!(TuiRenderer.change_mark(modified), "M");
    }

    /// The break this catches: a file's mark read from the whole name or a cased extension, a
    /// language's hue crossed with another's, or the folder's mark taken at this terminal's tier,
    /// so the phone's tree stops matching the TUI's.
    #[test]
    fn tree_marks_are_the_tui_language_marks() {
        use luxe_remote::MarkHue;
        use luxe_tui::glyphs::{GlyphTier, Icon};
        let nerd = |icon: Icon| icon.at(GlyphTier::Nerd).to_string();
        luxe_tui::glyphs::set_tier(GlyphTier::Ascii);
        let rust = TuiRenderer.file_mark("main.RS");
        let toml = TuiRenderer.file_mark("Cargo.toml");
        let folder = TuiRenderer.folder_mark();
        luxe_tui::glyphs::set_tier(GlyphTier::Nerd);
        assert_eq!(
            rust,
            luxe_remote::FileMark { glyph: nerd(Icon::LangRust), hue: MarkHue::Warning }
        );
        assert_eq!(
            toml,
            luxe_remote::FileMark { glyph: nerd(Icon::LangToml), hue: MarkHue::Error }
        );
        assert_eq!(TuiRenderer.file_mark("Makefile").hue, MarkHue::Muted);
        assert_eq!(folder, nerd(Icon::Folder));
    }

    /// The break this catches: the project panel's branch drawn with another mark than the TUI's
    /// project section, or at this terminal's tier rather than the phone's.
    #[test]
    fn the_branch_mark_is_the_tui_branch_mark() {
        use luxe_tui::glyphs::{GlyphTier, Icon};
        luxe_tui::glyphs::set_tier(GlyphTier::Ascii);
        let branch = TuiRenderer.branch_mark();
        luxe_tui::glyphs::set_tier(GlyphTier::Nerd);
        assert_eq!(branch, Icon::Branch.at(GlyphTier::Nerd));
    }

    /// The break this catches: a GitHub mark taken at this terminal's tier or crossed with
    /// another, ages in other words than the TUI's, or a body's HTML left as tags.
    #[test]
    fn github_reaches_the_tui_marks_and_words() {
        use luxe_tui::glyphs::{GlyphTier, Icon, Mark};
        luxe_tui::glyphs::set_tier(GlyphTier::Ascii);
        let marks = TuiRenderer.github_marks();
        luxe_tui::glyphs::set_tier(GlyphTier::Nerd);
        assert_eq!(marks.pull_request, Icon::PullRequest.at(GlyphTier::Nerd));
        assert_eq!(marks.issue, Icon::Issue.at(GlyphTier::Nerd));
        assert_eq!(marks.yes, Mark::Yes.at(GlyphTier::Nerd));
        assert_eq!(marks.no, Mark::No.at(GlyphTier::Nerd));
        assert_eq!(marks.waiting, Mark::Waiting.at(GlyphTier::Nerd));
        assert_eq!(marks.elision, Mark::Elision.at(GlyphTier::Nerd));
        assert_eq!(TuiRenderer.ago(1_000, 1_000 + 3 * 3600), "3h ago");
        let body = TuiRenderer.github_readable("<details><summary>Notes</summary>x</details>");
        assert!(!body.contains('<'), "{body}");
    }

    /// The break this catches: the bridge's default (no icon) left in place, so the home screen
    /// gets a screenshot of the page instead of the mark, or the size asked for not passed on.
    #[test]
    fn the_icon_is_the_tui_tile_at_the_size_asked_for() {
        let colors = luxe_remote::TileColors { plate: [0; 3], ink: [0xff; 3] };
        let png = TuiRenderer.home_icon(180, colors).expect("an icon");
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "a PNG");
        // the header chunk's width and height, big-endian, after the signature and chunk head
        assert_eq!(png[16..24], [0, 0, 0, 180, 0, 0, 0, 180]);
    }

    /// The break this catches: facts dropped or crossed on the way to the TUI, so the phone's
    /// status line misses the context reading, or colors a full window as a roomy one.
    #[test]
    fn status_facts_reach_the_tui_and_come_back_as_its_segments() {
        let facts: luxe_remote::StatusFacts = serde_json::from_value(serde_json::json!({
            "model": {"provider": "anthropic", "model": "claude-x"},
            "thinking": "medium",
            "context": {"used_tokens": 950, "context_window": 1000},
        }))
        .expect("the app's status facts parse");
        let segments = TuiRenderer.status(&facts);
        let words: Vec<_> = segments.iter().map(|s| (s.kind, s.label.as_str(), s.tone)).collect();
        assert_eq!(
            words,
            [
                ("context", "95.0%/1.0k", luxe_remote::StatusTone::Error),
                ("thinking", "med", luxe_remote::StatusTone::Plain),
                ("model", "claude-x", luxe_remote::StatusTone::Plain),
            ]
        );
    }
}
