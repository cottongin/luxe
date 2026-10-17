//! The built-ins — every command Luxe ships.

use super::{id, Command, Need};
use luxe_core::keymap::UiAction;

/// Every command Luxe ships, in no particular order: a menu sorts them.
///
/// Left out on purpose, because they are keys rather than commands and the key card lists them:
/// moving around a review or a diff, block selection, scrolling, the editor's own mechanics
/// (submit, newline, paste, select all), and anything that acts on "the selected" thing a palette
/// opened from anywhere cannot point at.
pub(super) fn built_in() -> Vec<Command> {
    use Need::{
        ChangesPane, Chat, Diff, Image, PullRequestReview, Repository, Review, Turn, Waiting,
    };
    vec![
        // === Anywhere ===
        Command::action(UiAction::Quit)
            .with_slash("/quit")
            .summary("Leaves Luxe; the session is saved as it goes, so nothing is lost.")
            .keywords(&["exit", "close"]),
        Command::slash(id::UPDATE, "Update Luxe", "/update")
            .summary("Quits Luxe and updates it with the command for however you installed it.")
            .detail(
                "The same as `luxe update`: each command is shown before it runs, in this \
                 terminal. A copy your system's package manager owns is left to it.",
            )
            .keywords(&["upgrade", "version", "release", "new"]),
        Command::action(UiAction::Reload)
            .with_slash("/reload")
            .summary("Re-reads your config and keybindings without restarting.")
            .keywords(&["refresh", "settings", "keybindings"]),
        Command::slash(id::HELP, "Help", "/help")
            .keys(UiAction::ToggleHelp)
            .summary("Every key for this screen, as your keymap binds them, and where the guide is.")
            .detail("Commands are in this list. In a review, the review's keys.")
            .keywords(&["help", "keys", "shortcuts", "keybindings"]),
        // === The conversation ===
        //
        // A review draws nothing over itself but the palette, so a command that opens a picker or
        // answers in the transcript lists here, where what it does can be seen
        Command::slash(id::MODEL, "Change model", "/model")
            .needs(&[Chat])
            .summary("Opens the model picker; the session carries on with whichever you choose.")
            .detail(
                "Every model Luxe knows for the providers it can reach, yours first. The context \
                 window, output cap, reasoning levels and pricing move with it.",
            )
            .keywords(&["switch", "provider", "llm"]),
        Command::slash(id::THINKING, "Set thinking level", "/thinking")
            .needs(&[Chat])
            .summary("How hard the model reasons before it answers, from Off to Ultra.")
            .detail(
                "A level the active model cannot honor is shown and cannot be picked. The level is \
                 remembered per model.",
            )
            .keywords(&["reasoning", "effort"]),
        Command::slash(id::THEME, "Change theme", "/theme")
            .needs(&[Chat])
            .summary("Previews every theme as you move, and keeps the one you choose.")
            .detail("`enter` writes it to your config. `esc` puts back the one you started with.")
            .keywords(&["colors", "appearance"]),
        Command::slash(id::MOUSE, "Mouse support", "/mouse")
            .needs(&[Chat])
            .keys(UiAction::ToggleMouse)
            .summary("Turns mouse capture on or off.")
            .detail(
                "On, the wheel scrolls, a drag selects and copies, and a click opens a card. Off \
                 gives every gesture back to the terminal. Hold Shift to hand one drag back without \
                 turning it off.",
            )
            .usage(&[("/mouse", "toggle"), ("/mouse on", ""), ("/mouse off", "")])
            .keywords(&["click", "scroll", "select"]),
        Command::slash(id::PERMISSIONS, "Auto-approve tool asks", "/permissions")
            .needs(&[Chat])
            .summary("Approves every tool call that would have asked you first.")
            .detail(
                "Deny rules still apply, and the statusline says it is on for as long as it is.",
            )
            .usage(&[("/permissions", "toggle"), ("/permissions on", ""), ("/permissions off", "")])
            .keywords(&["permission", "approve", "walk away"]),
        Command::slash(id::NEW, "New session", "/new")
            .needs(&[Chat])
            .summary("Starts a fresh session; the one you leave is kept, and can be resumed.")
            .detail("Resume it with `/sessions`, from the start screen, or with `luxe -c`.")
            .keywords(&["clear", "reset", "start"]),
        Command::slash(id::SESSIONS, "Sessions", "/sessions")
            .needs(&[Chat])
            .summary("Every session in this project, searchable, to pick up where it left off.")
            .detail(
                "Searches titles and what was said, with a preview of the one under the cursor; \
                 `#146` finds the sessions that worked on that pull request. The session you leave \
                 is kept, and one with a worktree of its own opens there.",
            )
            .keywords(&["resume", "switch", "history", "open", "find"]),
        Command::slash(id::MODE, "Switch mode", "/mode")
            .needs(&[Chat])
            .keys(UiAction::CycleMode)
            .summary("Moves between build, which changes things, and plan, which writes only the plan.")
            .detail(
                "Plan mode reads, asks and writes only `PLAN.md` (`plan.files`), and is held to it: \
                 an edit anywhere else does not run. Bare, `/mode` moves to the next mode. The \
                 composer's title shows the mode, and clicking it switches.",
            )
            .usage(&[("/mode", "the next mode"), ("/mode plan", ""), ("/mode build", "")])
            .keywords(&["plan", "build", "read-only", "planning", "agent"]),
        Command::slash(id::CONTEXT, "Context window", "/context")
            .needs(&[Chat])
            .summary("What is occupying the context window, ranked, with what to do about each.")
            .keywords(&["tokens", "occupancy"]),
        Command::slash(id::COMPACT, "Compact the conversation", "/compact")
            .needs(&[Chat])
            .summary("Summarizes the conversation now, whatever the occupancy.")
            .usage(&[("/compact", ""), ("/compact <focus>", "what the summary should keep")])
            .keywords(&["summarize", "summary", "shrink"]),
        Command::slash(id::COST, "Session cost", "/cost")
            .needs(&[Chat])
            .summary("What this session has cost, by model and by kind of token.")
            .detail("On a subscription it shows the allowance left rather than dollars.")
            .keywords(&["tokens", "price", "billing"]),
        Command::action(UiAction::SearchConvo)
            .with_slash("/search")
            .needs(&[Chat])
            .summary("Finds text in the transcript.")
            .keywords(&["find"]),
        Command::action(UiAction::CopyMessage)
            .needs(&[Chat, Turn])
            .summary("Copies the selected message, or the last one, to the clipboard.")
            .detail("With text selected, it copies the selection instead.")
            .keywords(&["clipboard", "yank"]),
        Command::action(UiAction::Retry)
            .needs(&[Chat, Turn])
            .summary("Sends your last message again, after the reply it got.")
            .detail(
                "The model sees that reply. To ask again as if it never happened, use Replace the \
                 last answer.",
            )
            .keywords(&["retry", "resend"]),
        Command::action(UiAction::Regenerate)
            .needs(&[Chat, Turn])
            .summary("Asks your last message again in its place, without the reply it replaces.")
            .keywords(&["regenerate", "redo"]),
        Command::action(UiAction::EditResend)
            .needs(&[Chat, Turn])
            .summary("Puts your last message back in the composer to change and send again.")
            .keywords(&["edit", "resend"]),
        Command::action(UiAction::DequeueMessages)
            .needs(&[Chat, Waiting])
            .summary("Brings every message still waiting for a turn back into the composer.")
            .keywords(&["queue", "recall"]),
        Command::action(UiAction::ToggleToolOutput)
            .needs(&[Chat, Turn])
            .summary("Expands every tool card to its whole output, or folds them back.")
            .keywords(&["expand", "collapse", "tools"]),
        Command::action(UiAction::OpenImage)
            .needs(&[Chat, Image])
            .summary("Opens the newest image in the conversation full-window.")
            .keywords(&["picture", "screenshot"]),
        // === The workspace ===
        Command::slash(id::DOCK, "Context Dock", "/dock")
            .needs(&[Chat])
            .keys(UiAction::ToggleDock)
            .summary("Shows or hides the sidebar.")
            .detail(
                "`auto` hands it back to the layout, which shows it when there is room. With the \
                 statusline on `auto`, hiding the sidebar brings the statusline back, so this \
                 swaps one for the other.",
            )
            .usage(&[
                ("/dock", "toggle"),
                ("/dock show", ""),
                ("/dock hide", ""),
                ("/dock auto", ""),
                ("/dock left", ""),
                ("/dock right", ""),
            ])
            .keywords(&["sidebar", "panel", "rail"]),
        Command::action(UiAction::ToggleDockPosition)
            .needs(&[Chat])
            .summary("Moves the Context Dock to the other side of the conversation.")
            .keywords(&["dock", "left", "right"]),
        Command::slash(id::STATUSLINE, "Statusline", "/statusline")
            .needs(&[Chat])
            .keys(UiAction::ToggleStatusline)
            .summary("Shows or hides the bar under the composer.")
            .detail(
                "`auto` shows it only when the sidebar is not beside the conversation, so the \
                 telemetry is on screen once.",
            )
            .usage(&[
                ("/statusline", "toggle"),
                ("/statusline show", ""),
                ("/statusline hide", ""),
                ("/statusline auto", ""),
            ])
            .keywords(&["bar", "footer", "status"]),
        Command::slash(id::CHANGES, "Preview", "/preview")
            .needs(&[Chat])
            .keys(UiAction::ToggleChanges)
            .summary("Shows one file where the transcript is, whole or as its diff.")
            .detail(
                "Read-only: nothing is written, no model is asked, and looking does not mark a \
                 file reviewed.",
            )
            .usage(&[
                ("/preview", "the file the sidebar leads with"),
                ("/preview <path>", "that file"),
            ])
            .keywords(&["peek", "files", "changes", "what changed", "git diff", "view"]),
        Command::slash(id::EDITOR, "Open in editor", "/editor")
            .needs(&[Chat])
            .summary("Opens the project, or one file or folder, in your editor.")
            .detail(
                "The editor is `$LUXE_EDITOR`, `$VISUAL` or `$EDITOR`. A terminal editor takes the \
                 screen until it exits; the agent keeps working underneath. In the preview and in a \
                 review, the file on screen opens at the line you are on.",
            )
            .usage(&[
                ("/editor", "the project"),
                ("/editor <path>", "a file or folder, relative to the project"),
                ("/editor <path>:<line>", "a file, at that line"),
            ])
            .keywords(&["open", "edit", "vim", "code", "file"]),
        Command::action(UiAction::ChangesOpenInEditor)
            .needs(&[ChangesPane])
            .summary("Opens the previewed file in your editor, at the line you are on.")
            .keywords(&["edit", "open"]),
        Command::action(UiAction::ChangesToggleView)
            .needs(&[ChangesPane])
            .summary("Shows a changed file whole, or what changed in it.")
            .keywords(&["diff", "file", "view"]),
        Command::action(UiAction::AskAboutSelection)
            .needs(&[ChangesPane])
            .summary("Puts the lines you picked in the File view into the composer, to ask about.")
            .detail(
                "Drag across line numbers or code to pick lines, or right-click them. The agent \
                 gets the path and the lines, and reads them itself.",
            )
            .keywords(&["ask", "lines", "selection", "mention"]),
        Command::action(UiAction::FocusFiles)
            .needs(&[Chat])
            .summary("Moves the keyboard into the dock's file tree.")
            .detail(
                "`j`/`k` move, `l` opens, `h` closes, `enter` shows a file, `/` filters, `o` opens \
                 it in your editor, `@` mentions it, `y` copies its path, `a` shows only what \
                 changed, `esc` goes back to the composer.",
            )
            .keywords(&["files", "tree", "explorer", "browse"]),
        Command::action(UiAction::ReviewOpenInEditor)
            .needs(&[Review])
            .summary("Opens the file under the cursor in your editor, at its line.")
            .detail("It opens the working tree's copy, which is the one an editor can change.")
            .keywords(&["edit", "open"]),
        Command::action(UiAction::ChangesToggleWordWrap)
            .needs(&[ChangesPane])
            .summary("Wraps long lines in the diff, or lets them run off the edge.")
            .keywords(&["wrap"]),
        Command::action(UiAction::ReviewToggleSplit)
            .needs(&[Diff])
            .summary("Draws the diff in two columns, or in one.")
            .detail("It starts from `diff.layout`, and this overrides that for the session.")
            .keywords(&["split", "unified", "layout"]),
        Command::slash(id::WORKTREE, "Worktree", "/worktree")
            .needs(&[Chat])
            .summary("Where this session works, and whether it is a checkout of its own.")
            .detail("`luxe --worktree` starts a session in a worktree of its own. The branch stays.")
            .usage(&[
                ("/worktree", ""),
                ("/worktree remove", "end the session and remove its worktree"),
            ])
            .keywords(&["checkout", "branch", "git"]),
        // === Reviewing ===
        Command::slash(id::DIFF, "Review changes", "/diff")
            .needs(&[Chat])
            .keys(UiAction::ReviewOpen)
            .summary("Opens a Guided Review: a walkthrough of the current revision, read one chapter at a time.")
            .detail(
                "Used alone, `/diff` creates a review for the working tree, or opens it if one is \
                 already pinned. You can optionally name a target to create a fresh review for it.",
            )
            .usage(&[
                ("/diff", "the review open, or the working tree"),
                ("/diff turn", "the last turn"),
                ("/diff base <ref>", ""),
                ("/diff commit <oid>", ""),
                ("/diff pr <number or URL>", ""),
                ("/diff refresh", "rebuild the open review"),
            ])
            .keywords(&["review", "guided"]),
        Command::slash(id::REVIEW, "Second opinion", "/review")
            .needs(&[Chat])
            .summary("A reviewer in a fresh context reads the uncommitted changes and tells the agent.")
            .detail(
                "The reviewer changes nothing and never sees this conversation, which is what makes \
                 it a second opinion. It runs on `reviewer.model`, or this session's model. Findings \
                 come back as the agent's next turn, and the agent may push back on one it thinks \
                 is wrong.",
            )
            .usage(&[("/review", ""), ("/review <focus>", "what to look at hardest")])
            .keywords(&["reviewer", "second opinion", "check", "critique"]),
        Command::slash(id::PR, "Pull request", "/pr")
            .needs(&[Chat, Repository])
            .summary("This branch's pull request: status, reviewers and conversation.")
            .detail(
                "Each Status row carries its key; Enter on Ready opens the merge. In the \
                 Conversation, `r` replies and `d` drafts a reply with the agent.",
            )
            .keywords(&["checks", "threads", "github"]),
        Command::slash(id::GITHUB, "GitHub", "/github")
            .needs(&[Chat])
            .summary("Pull requests and issues on GitHub, with each one's status and conversation.")
            .detail(
                "Sections come from `github.sections`. Enter reviews or takes up the row, `→` \
                 moves to its Overview and Conversation, `c` refers to it, `o` opens it, and `/` \
                 filters.",
            )
            .keywords(&["pull requests", "issues", "gh"]),
        Command::slash(id::STACK, "Restack the stack", "/stack")
            .needs(&[Chat, Repository])
            .summary("Moves every branch in the stack back onto the one below it.")
            .detail(
                "Planned with `git replay` before anything moves, shown to you, then pushed \
                 atomically with leases.",
            )
            .keywords(&["rebase", "stacked", "branches"]),
        Command::action(UiAction::ReviewReturnChat)
            .needs(&[Review])
            .summary("Back to the conversation, with the review kept pinned for when you return.")
            .keywords(&["exit", "close", "chat"]),
        Command::action(UiAction::ReviewFinish)
            .needs(&[Review])
            .summary("Decides what happens to your comments: send them, commit, or open a pull request.")
            .detail("For a pull request, it posts the review to GitHub.")
            .keywords(&["done", "accept"]),
        Command::action(UiAction::ReviewRefresh)
            .needs(&[Review])
            .summary("Rebuilds the review from its source, so it shows the latest changes.")
            .keywords(&["reload"]),
        Command::action(UiAction::ReviewSource)
            .needs(&[Review])
            .summary("Reviews something else: the working tree, the last turn, a base, a commit.")
            .keywords(&["target", "base", "commit"]),
        Command::action(UiAction::ReviewAddFinding)
            .needs(&[Review])
            .summary("Writes a comment on the line under the cursor, for the agent to act on.")
            .detail("In a pull request's review, it is posted with the review instead.")
            .keywords(&["finding", "note"]),
        Command::action(UiAction::ReviewAskQuestion)
            .needs(&[Review])
            .summary("Asks the agent about the line under the cursor; it answers and never edits.")
            .keywords(&["question"]),
        Command::action(UiAction::ReviewFindings)
            .needs(&[Review])
            .summary("Every comment on this review, to send, reply to or verify.")
            .keywords(&["findings", "feedback"]),
        Command::action(UiAction::ReviewSend)
            .needs(&[Review])
            .summary("Sends every unsent comment to the agent to act on.")
            .keywords(&["submit", "agent"]),
        Command::action(UiAction::ReviewOpenInBrowser)
            .needs(&[PullRequestReview])
            .summary("Opens this pull request in the browser.")
            .keywords(&["browser", "web"]),
        Command::action(UiAction::ReviewToggleGuide)
            .needs(&[Review])
            .summary("Shows or hides the guide beside the diff.")
            .keywords(&["prose", "chapters"]),
        Command::action(UiAction::ReviewToggleNoise)
            .needs(&[Review])
            .summary("Opens or folds the churn the review collapsed, like lockfiles and moves.")
            .keywords(&["noise", "generated"]),
        Command::action(UiAction::ReviewToggleWordWrap)
            .needs(&[Review])
            .summary("Wraps long lines in the diff, or lets them run off the edge.")
            .keywords(&["wrap"]),
        // === Status, setup and signing in ===
        Command::slash(id::LSP, "Language servers", "/lsp")
            .needs(&[Chat])
            .summary("Every configured language server, whether it started, and what it is doing.")
            .keywords(&["diagnostics"]),
        Command::slash(id::DEBUG, "Debug adapters", "/debug")
            .needs(&[Chat])
            .summary("Every configured debug adapter, and whether it can be reached.")
            .keywords(&["dap", "debugger"]),
        Command::slash(id::MCP, "MCP servers", "/mcp")
            .needs(&[Chat])
            .summary("Every configured MCP server, its transport, and the tools it exposes.")
            .keywords(&["servers"]),
        Command::slash(id::TOOLS, "Tools", "/tools")
            .needs(&[Chat])
            .summary("Every tool the agent can reach this run, and the ones it cannot.")
            .keywords(&["capabilities"]),
        Command::slash(id::IMAGES, "Inline images", "/images")
            .needs(&[Chat])
            .summary("What this terminal can draw, and why.")
            .keywords(&["graphics", "pictures", "kitty"]),
        Command::slash(id::LOGIN, "Sign in", "/login")
            .needs(&[Chat])
            .summary("Adds a credential for a provider.")
            .detail("Bare, it lists the providers. With a name, it goes straight to that one.")
            .usage(&[("/login", ""), ("/login <provider>", "")])
            .keywords(&["api key", "credentials"]),
        Command::slash(id::LOGOUT, "Sign out", "/logout")
            .needs(&[Chat])
            .summary("Removes the credential Luxe stored for a provider.")
            .detail("Bare, it lists what is signed in.")
            .usage(&[("/logout <provider>", "")])
            .keywords(&["credentials"]),
        Command::slash(id::AUTH, "Credentials", "/auth")
            .needs(&[Chat])
            .summary("Which providers you are signed in to, and how.")
            .keywords(&["login", "api key"]),
        Command::slash(id::CONFIG, "Configuration", "/config")
            .needs(&[Chat])
            .summary("Providers, keys, settings and the config files, in one surface.")
            .keywords(&["setup", "providers", "login"]),
        Command::slash(id::SETTINGS, "Settings", "/settings")
            .needs(&[Chat])
            .summary("Appearance, the conversation, the dock, tools and the agent, as controls.")
            .detail(
                "Changes are written to your `config.json`, keeping everything else in it. `e` \
                 opens the file, and a value this project's `.luxe/config.json` sets is shown but \
                 changed there.",
            )
            .keywords(&["preferences", "options", "tools", "config"]),
    ]
}
