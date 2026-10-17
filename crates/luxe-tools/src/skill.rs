// Skill discovery + load tool.
//
// skill — discover SKILL.md playbooks from global (~/.config/luxe/skills/) and project
// (./.luxe/skills/) directories, and load one's full instructions on demand. Discovery + parsing
// live in `luxe_core::skills` (shared with the system-prompt builder); this tool is the surface.

use async_trait::async_trait;
use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData};

/// Skill discovery + load tool.
pub(crate) struct SkillTool;

/// Configuration placeholder kept for API compatibility (discovery dirs are resolved by
/// [`luxe_core::skills`]).
#[derive(Debug, Clone, Default)]
pub(crate) struct SkillConfig;

impl SkillTool {
    pub(crate) fn new(_config: SkillConfig) -> Self {
        Self
    }
}

/// Ceiling on one loaded skill body.
const SKILL_MAX_CHARS: usize = 40_000;

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "skill".to_string(),
            description: "Discover and load skills — SKILL.md playbooks from ~/.config/luxe/skills \
                and ./.luxe/skills. `action:\"discover\"` lists available skills (name + \
                description); `action:\"load\"` with `name` returns that skill's full instructions. \
                Load a skill before doing work it covers."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["discover", "load"], "description": "discover = list skills; load = fetch one skill's instructions"},
                    "name": {"type": "string", "description": "Skill name (required for load)"}
                },
                "required": ["action"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'action' argument is required");
        };
        match action {
            "discover" => {
                let skills: Vec<serde_json::Value> = luxe_core::skills::discover(&cx.cwd)
                    .into_iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "description": s.description,
                            "path": luxe_core::paths::display(&s.path),
                        })
                    })
                    .collect();
                if skills.is_empty() {
                    return ToolResultData::text(format!(
                        "No skills found. Add one at .luxe/skills/<name>/SKILL.md (with \
                             `name` + `description` frontmatter) or {}/.",
                        luxe_core::user_dirs::config_path_display("skills")
                    ));
                }
                ToolResultData::text(serde_json::to_string_pretty(&skills).unwrap_or_default())
            }
            "load" => {
                let Some(name) =
                    args.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                else {
                    return ToolResultData::error("Error: 'name' is required for load");
                };
                match luxe_core::skills::load_with_path(&cx.cwd, name) {
                    // A skill is authored to be loaded, so the ceiling is generous — it exists so
                    // one oversized file cannot swallow the context window it was meant to inform.
                    // The path goes in the hint because "read the file" is useless without it.
                    Some((body, path)) => {
                        let body = crate::output_limit::cap_chars(
                            &body,
                            SKILL_MAX_CHARS,
                            &format!(
                                "Read the rest with: read {}",
                                luxe_core::paths::display(&path)
                            ),
                        );
                        // a skill's reference files are named relative to its own folder
                        let folder = path.parent().map(luxe_core::paths::display);
                        ToolResultData::text(match folder {
                            Some(folder) => {
                                format!("{body}\n\nFiles this skill names are in {folder}")
                            }
                            None => body,
                        })
                    }
                    None => ToolResultData::error(format!(
                        "No skill named '{name}'. Use action:\"discover\" to list skills."
                    )),
                }
            }
            other => ToolResultData::error(format!(
                "Unknown action '{other}'. Use 'discover' or 'load'."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{mpsc, watch};

    fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
        let (event_tx, _event_rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        ToolContext {
            cwd: tmp_dir.to_path_buf(),
            session_id: "test-session".to_string(),
            cancelled: Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        }
    }

    fn text_of(result: &ToolResultData) -> String {
        match &result.content[0] {
            luxe_core::ContentBlock::Text(s) => s.clone(),
            _ => panic!("expected text"),
        }
    }

    #[tokio::test]
    async fn discover_reports_none_when_empty() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path()); // isolate global skills (nextest = process-per-test)
        std::env::remove_var("XDG_CONFIG_HOME");
        let tool = SkillTool::new(SkillConfig);
        let result =
            tool.run(serde_json::json!({"action": "discover"}), &test_context(tmp.path())).await;
        assert!(!result.is_error);
        assert!(text_of(&result).contains("No skills found"));
    }

    #[tokio::test]
    async fn discover_then_load_a_project_skill() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());
        std::env::remove_var("XDG_CONFIG_HOME");
        let sd = tmp.path().join(".luxe/skills/test-skill");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(
            sd.join("SKILL.md"),
            "---\nname: Test Skill\ndescription: A test skill\n---\n\n# Test Skill\n\nBody here.",
        )
        .unwrap();
        let tool = SkillTool::new(SkillConfig);
        let ctx = test_context(tmp.path());

        let discovered = tool.run(serde_json::json!({"action": "discover"}), &ctx).await;
        assert!(text_of(&discovered).contains("Test Skill"));

        let loaded =
            tool.run(serde_json::json!({"action": "load", "name": "Test Skill"}), &ctx).await;
        assert!(!loaded.is_error);
        let body = text_of(&loaded);
        assert!(body.contains("Body here."));
        assert!(!body.contains("description:"), "frontmatter stripped");
        // A skill that names `references/x.md` is no use unless the agent knows where it lives
        let folder = luxe_core::paths::display(&sd);
        assert!(body.contains(&format!("Files this skill names are in {folder}")), "{body}");
    }

    #[tokio::test]
    async fn load_unknown_and_missing_args_error() {
        let tmp = TempDir::new().unwrap();
        std::env::set_var("HOME", tmp.path());
        let tool = SkillTool::new(SkillConfig);
        let ctx = test_context(tmp.path());
        assert!(tool.run(serde_json::json!({}), &ctx).await.is_error);
        assert!(tool.run(serde_json::json!({"action": "load"}), &ctx).await.is_error);
        assert!(
            tool.run(serde_json::json!({"action": "load", "name": "nope"}), &ctx).await.is_error
        );
    }
}
