//! Engine restacking — a stack's branches moved back onto the ones below them, planned then
//! applied.
//!
//! Planning reads the stack the branch's pull request is in from GitHub, asks what merged out
//! from under its bottom, and has [`crate::restack::plan`] make the new commits; the front-end
//! shows that plan, and applying pushes exactly it, then retargets the pull requests whose base
//! moved. The plan stays here between the two, so the push names it rather than carrying it.

use super::posting::sentence;
use super::*;
use crate::github::{GitHub, PullRequestState};
use crate::restack::{ApplyError, Layer, MergedBelow, Planned, RestackId};
use std::path::PathBuf;

impl Engine {
    /// Works out what restacking the checked-out branch's stack would do, and keeps the plan.
    pub(crate) fn handle_plan_restack(
        &self,
        request_id: RestackId,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let previews = Arc::clone(&self.previews);
        super::previews::lock(&previews).restack.begin(request_id.clone());
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match plan(github, root).await {
                Ok((planned, merged)) => {
                    let plan = Some(planned.plan.clone());
                    super::previews::lock(&previews).restack.keep(&request_id, planned);
                    Event::RestackPlanned { request_id, plan, merged, refusal: None }
                }
                Err(refusal) => Event::RestackPlanned {
                    request_id,
                    plan: None,
                    merged: None,
                    refusal: Some(refusal),
                },
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Pushes the plan `request_id` names, then retargets the pull requests whose base moved.
    pub(crate) fn handle_restack(&self, request_id: RestackId, event_tx: &mpsc::Sender<Event>) {
        let github = self.config.github.clone();
        let root = self.config.guided_review_project_root.clone();
        let plan = super::previews::lock(&self.previews).restack.take(&request_id);
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            let event = match plan {
                None => Event::RestackFailed {
                    request_id,
                    message: "That plan is not the current one, or was already pushed, so \
                              nothing was pushed. Plan the restack again."
                        .into(),
                },
                Some(plan) => match apply(github, root, &plan).await {
                    Ok((moved, retargeted, retarget_failed)) => {
                        Event::Restacked { request_id, moved, retargeted, retarget_failed }
                    }
                    Err(message) => Event::RestackFailed { request_id, message },
                },
            };
            let _ = event_tx.send(event).await;
        });
    }
}

async fn plan(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
) -> Result<(Planned, Option<MergedBelow>), String> {
    let github = github.ok_or("Restacking needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Restacking needs a project, and this run has none.")?;
    let repository = github.repository(&root).await.map_err(|error| sentence(&error))?;
    let head = crate::github::branch_head(&root)
        .await
        .ok_or("This checkout is not on a branch, so it is in no stack.")?;
    let status =
        github.branch_status(&repository, &head).await.map_err(|error| sentence(&error))?;
    let pr =
        status.pull_request.filter(|pr| pr.state == PullRequestState::Open).ok_or_else(|| {
            format!("`{}` has no open pull request, so it is in no stack.", head.name)
        })?;
    let (layers, merged) = match pr.stack.as_ref() {
        Some(stack) => {
            let layers = layers(pr.number, &head.name, stack);
            let bottom = &layers[0];
            let merged = github
                .merged_below(&repository, bottom.number, &bottom.base_ref)
                .await
                .map_err(|error| sentence(&error))?;
            (layers, merged)
        }
        // The last layer left once the one below merged: nothing open is under it any more, so
        // the stack reads as gone, but its branch still carries the merged commits
        None => {
            let base = github
                .pull_request(&repository, pr.number)
                .await
                .map_err(|error| sentence(&error))?
                .base_ref;
            let merged = match base == repository.default_branch {
                true => None,
                false => github
                    .merged_below(&repository, pr.number, &base)
                    .await
                    .map_err(|error| sentence(&error))?,
            };
            let lone = || {
                format!(
                    "#{} is not in a stack: nothing is stacked on it, and it is on nothing.",
                    pr.number
                )
            };
            let merged = merged.ok_or_else(lone)?;
            let layer = Layer { number: pr.number, head_ref: head.name.clone(), base_ref: base };
            (vec![layer], Some(merged))
        }
    };
    let remote = repository.remote.clone().unwrap_or_else(|| "origin".to_string());
    let plan = crate::restack::plan(&root, &remote, &layers, merged.as_ref())
        .await
        .map_err(|error| capitalized(&error.to_string()))?;
    Ok((plan, merged))
}

/// The stack as layers, bottom first: each one's base is the branch of the one below, and the
/// bottom's is the branch the stack lands on.
fn layers(number: u64, branch: &str, stack: &crate::github::Stack) -> Vec<Layer> {
    let mut layers = Vec::new();
    let mut base = stack.trunk.clone();
    let entries = stack.below.iter().map(|entry| (entry.number, entry.head_ref.as_str()));
    let here = std::iter::once((number, branch));
    let above = stack.above.iter().map(|entry| (entry.number, entry.head_ref.as_str()));
    for (number, head) in entries.chain(here).chain(above) {
        layers.push(Layer { number, head_ref: head.to_string(), base_ref: base });
        base = head.to_string();
    }
    layers
}

/// Pushes the plan, then retargets the pull requests whose base moved.
///
/// The repository is found first, so nothing after a push can fail for want of it. A failure once
/// the branches are pushed is not an error: they moved, the retargets still follow, and what did
/// not happen here is said in `retarget_failed`.
async fn apply(
    github: Option<Arc<dyn GitHub>>,
    root: Option<PathBuf>,
    planned: &Planned,
) -> Result<(Vec<u64>, Vec<u64>, Option<String>), String> {
    let github = github.ok_or("Restacking needs the GitHub CLI, and this run cannot reach it.")?;
    let root = root.ok_or("Restacking needs a project, and this run has none.")?;
    let repository = github
        .repository(&root)
        .await
        .map_err(|error| format!("{} Nothing was pushed.", sentence(&error)))?;
    let mut said = match crate::restack::apply(&root, planned).await {
        Ok(()) => Vec::new(),
        Err(failure) => vec![after_push(failure)?],
    };
    let moved =
        planned.steps.iter().filter(|step| step.new != step.old).map(|s| s.number).collect();
    let mut retargeted = Vec::new();
    for step in &planned.steps {
        let Some(base) = &step.retarget else {
            continue;
        };
        // Pushed already: a base left unmoved is said, not undone
        if let Err(error) = github.retarget_pull_request(&repository, step.number, base).await {
            let opening = if said.is_empty() { "The branches were pushed, but #" } else { "And #" };
            said.push(format!(
                "{opening}{} could not be moved to `{base}`: {}",
                step.number,
                sentence(&error)
            ));
            break;
        }
        retargeted.push(step.number);
    }
    Ok((moved, retargeted, (!said.is_empty()).then(|| said.join(" "))))
}

/// What a failed apply says: an error when nothing is known to have been pushed, else the sentence
/// that goes with a publication that happened.
fn after_push(failure: ApplyError) -> Result<String, String> {
    match failure {
        ApplyError::Refused(error) => {
            Err(format!("{} Nothing was pushed.", capitalized(&error.to_string())))
        }
        ApplyError::PushFailed(error) => Err(format!(
            "{} The push did not finish, so check the branches on GitHub before planning again.",
            capitalized(&error.to_string())
        )),
        ApplyError::AfterPush(error) => Ok(format!(
            "The branches were pushed, but the ones here did not follow: {}",
            capitalized(&error.to_string())
        )),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    let opening: String =
        chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect());
    format!("{opening}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Stack, StackEntry};

    fn entry(number: u64, head: &str) -> StackEntry {
        StackEntry {
            number,
            title: String::new(),
            head_ref: head.into(),
            draft: false,
            checked: false,
        }
    }

    #[test]
    fn the_stack_becomes_layers_each_on_the_one_below() {
        let stack = Stack {
            trunk: "main".into(),
            trunk_is_default: true,
            below: vec![entry(1, "one"), entry(2, "two")],
            above: vec![entry(4, "four")],
        };
        let layers = layers(3, "three", &stack);
        let shape: Vec<(u64, &str, &str)> = layers
            .iter()
            .map(|layer| (layer.number, layer.head_ref.as_str(), layer.base_ref.as_str()))
            .collect();
        assert_eq!(
            shape,
            [(1, "one", "main"), (2, "two", "one"), (3, "three", "two"), (4, "four", "three")]
        );
        assert_eq!(capitalized("a conflict"), "A conflict.");
    }

    /// Only a refusal before the push may say nothing was pushed; a push that did not finish
    /// claims nothing, and one that landed is reported as landed.
    #[test]
    fn a_failed_apply_says_what_happened_to_the_push() {
        use crate::restack::RestackError;
        let error = || RestackError::Dirty { head_ref: "two".into(), path: "/wt".into() };
        let refused = after_push(ApplyError::Refused(error())).unwrap_err();
        assert!(refused.ends_with("Nothing was pushed."), "{refused}");
        let unsure = after_push(ApplyError::PushFailed(error())).unwrap_err();
        assert!(!unsure.contains("Nothing was pushed") && unsure.contains("check the branches"));
        let landed = after_push(ApplyError::AfterPush(error())).unwrap();
        assert!(landed.starts_with("The branches were pushed"), "{landed}");
    }
}
