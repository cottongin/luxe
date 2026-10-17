//! Workspace state — revision-safe project and activity reduction.
//!
//! The CLI collector owns project I/O. This reducer only accepts typed channel updates that
//! belong to the active session and advance their scope or per-call revision.

use super::App;
use luxe_core::config::ToolDetailsPreference;
use luxe_core::{ToolActivity, ToolActivityState, WorkspaceSnapshot};

pub(super) fn apply_snapshot(app: &mut App, snapshot: WorkspaceSnapshot) {
    if app.retired_workspace_scopes.contains(&snapshot.scope_id) {
        return;
    }

    let should_apply = app.workspace.as_ref().is_none_or(|current| {
        if current.scope_id == snapshot.scope_id {
            snapshot.revision > current.revision
        } else {
            app.retired_workspace_scopes.insert(current.scope_id.clone());
            true
        }
    });
    if should_apply {
        // A working tree that just emptied is the one state where FILES marking nothing would be
        // a lie by omission: the work went into a commit, `HEAD` moved with it, and the section
        // that was about to be reviewed vanishes with no sign it was ever there. Remembering the
        // transition is what lets the dock keep pointing at the turn that is still reviewable.
        //
        // But emptying alone does not mean committed. A revert empties it, and so does an agent
        // that writes a scratch file, uses it and deletes it — after which the dock announced
        // "committed — nothing in the working tree" over a repository whose history had not moved.
        // `HEAD` is the difference, so the claim is made only when it actually moved.
        let previous = app.workspace.as_ref();
        let had_changes = previous.is_some_and(|current| current.changes.total_files > 0);
        let head_moved = previous.is_some_and(|current| {
            current.project.git_head.is_some()
                && snapshot.project.git_head.is_some()
                && current.project.git_head != snapshot.project.git_head
        });
        app.worktree_emptied = if snapshot.changes.total_files > 0 {
            false
        } else {
            app.worktree_emptied || (had_changes && head_moved)
        };
        app.workspace = Some(snapshot);
    }
}

/// A card's label with the project's own absolute prefix taken off its paths.
///
/// A model that passes `/home/you/project/src/a.rs` gets the same file as one that passes
/// `src/a.rs`, and the card should read the same: the prefix is the one part every card would
/// repeat. A `bash` card is left alone, because its label is the command as it ran.
fn within_project(tool: &str, label: &str, root: &std::path::Path) -> String {
    // a filesystem root as the project would strip every separator
    if tool == "bash" || root.parent().is_none() {
        return label.to_string();
    }
    let forward = format!("{}/", luxe_core::paths::display(root).trim_end_matches('/'));
    let native = format!("{}{}", root.display(), std::path::MAIN_SEPARATOR);
    label.replace(&forward, "").replace(&native, "")
}

pub(super) fn apply_tool_activity(app: &mut App, mut activity: ToolActivity) {
    if app.session_id.as_deref() != Some(activity.session_id.as_str()) {
        return;
    }
    if let Some(root) = &app.project_root {
        activity.label = within_project(&activity.name, &activity.label, root);
    }
    let key = (activity.session_id.clone(), activity.call_id.clone());
    // A newer revision applies, except one that would move a settled card back. A call that runs
    // before its message commits settles first and learns its full label after, and that second
    // update keeps the state it already had.
    let should_apply = app.tool_activity.get(&key).is_none_or(|current| {
        activity.revision > current.revision
            && (!current.is_terminal() || activity.state == current.state)
    });
    if should_apply {
        app.tool_activity.insert(key.clone(), activity.clone());
        let preference = app.config.conversation.tool_details();
        let expanded = match preference {
            ToolDetailsPreference::All => true,
            ToolDetailsPreference::Errors => activity.state == ToolActivityState::Failed,
            ToolDetailsPreference::Collapsed | ToolDetailsPreference::Unknown => false,
        };
        let item = app
            .tool_items
            .entry(key)
            .and_modify(|item| {
                let expand_failed_activity = preference == ToolDetailsPreference::Errors
                    && !item.details_toggled
                    && item.activity.state != ToolActivityState::Failed
                    && activity.state == ToolActivityState::Failed;
                item.activity = activity.clone();
                if expand_failed_activity {
                    item.expanded = true;
                }
            })
            .or_insert(super::ToolItem {
                activity,
                expanded,
                details_toggled: false,
                waited_on_user_ms: 0,
            })
            .clone();
        // Housekeeping that went fine never reaches the transcript. The state above is still
        // recorded, so nothing downstream loses track of the call — it simply has no card.
        if !item.activity.shows_in_transcript() {
            return;
        }
        let id = super::ConversationItemId::Tool {
            session_id: item.activity.session_id.clone(),
            call_id: item.activity.call_id.clone(),
        };
        if let Some(existing) = app.convo.iter_mut().find(|existing| existing.id == id) {
            existing.set_block(super::ConvoBlock::Tool(Box::new(item)));
        } else {
            app.convo
                .push(super::ConversationItem::new(id, super::ConvoBlock::Tool(Box::new(item))));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::within_project;
    use std::path::Path;

    #[test]
    fn a_card_reads_the_same_however_the_model_spelled_the_path() {
        let root = Path::new("/tmp/rec-1/todo");
        assert_eq!(
            within_project("edit", "/tmp/rec-1/todo/test/a.js · 2 edits", root),
            "test/a.js · 2 edits"
        );
        assert_eq!(within_project("read", "/tmp/rec-1/todo/a.js, b.js", root), "a.js, b.js");
        assert_eq!(
            within_project("edit", "/tmp/rec-1/todo-two/a.js", root),
            "/tmp/rec-1/todo-two/a.js",
            "a sibling is not inside"
        );
        assert_eq!(within_project("edit", "/etc/hosts", root), "/etc/hosts");
        assert_eq!(
            within_project("bash", "cat /tmp/rec-1/todo/a.js", root),
            "cat /tmp/rec-1/todo/a.js",
            "a command stays as it ran"
        );
        assert_eq!(within_project("read", "/etc/hosts", Path::new("/")), "/etc/hosts");
    }
}
