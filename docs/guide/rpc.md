---
title: "The RPC protocol"
summary: "Drive Luxe from your own program: an editor plugin, a dashboard, or a front end of your own."
order: 7
group: "Extend"
---

`luxe --rpc` runs Luxe without its interface and speaks JSON over stdin and stdout, one object per
line. Use it to build an editor plugin, a dashboard, a CI adapter, or a whole new front end, in any
language.

```bash
luxe --rpc        # takes the usual flags: --model, --permission-mode, -c, --resume
luxe --rpc --github-writes   # also lets your client publish to GitHub
```

A client is trusted like the interface it replaces: it answers permission prompts, and it cannot
loosen any permission rule. Nothing listens on the network. Your client either started Luxe, or
[joined a session](#join-a-running-session) open in the interface on the same machine.

## A session

```text
→  {"kind":"ready","content":{"protocol_version":2,"luxe_version":"0.1.0","session_id":"…","cwd":"/work/app","github_writes":false}}
→  {"kind":"state_changed","content":{"model":{…},"streaming":false,"mode":"build","thinking":"off"}}
←  {"kind":"submit_user_message","content":{"text":"what does this repo do?"}}
→  {"kind":"turn_started"}
→  {"kind":"tool_started","content":{"call_id":"call_1","name":"read"}}
→  {"kind":"token_delta","content":{"block":0,"delta":"It is "}}
→  {"kind":"turn_usage","content":{"prompt_tokens":5018,"completion_tokens":26,"cost_usd":0.0258,…}}
→  {"kind":"turn_ended","content":{"reason":"end_turn"}}
←  {"kind":"close"}
```

Every frame is `{"kind": …, "content": …}`, and a frame with no payload leaves out `content`.
`ready` always comes first: check its `protocol_version` and refuse a number you do not know.

## A client

```python title="client.py"
import json, subprocess

luxe = subprocess.Popen(["luxe", "--rpc"], stdin=subprocess.PIPE,
                        stdout=subprocess.PIPE, text=True, bufsize=1)

def send(kind, content=None):
    frame = {"kind": kind} | ({"content": content} if content is not None else {})
    luxe.stdin.write(json.dumps(frame) + "\n")
    luxe.stdin.flush()

for line in luxe.stdout:
    event = json.loads(line)
    kind, body = event["kind"], event.get("content")

    if kind == "ready":
        assert body["protocol_version"] == 2, "unsupported protocol"
        send("submit_user_message", {"text": "What does this project do?"})
    elif kind == "token_delta":
        print(body["delta"], end="", flush=True)
    elif kind == "tool_started":
        print(f"\n[{body['name']}]")
    elif kind == "permission_requested":
        send("respond_permission", {"id": body["id"], "reply": "once"})
    elif kind == "question_asked":
        send("answer_question", {"id": body["id"], "reply": {"answered": ["yes"]}})
    elif kind == "action_rejected":
        print(f"\n! refused {body.get('kind')}: {body['reason']}")
    elif kind == "turn_ended":
        print()
        send("close")
    # Ignore everything else, so the client keeps working with newer versions of Luxe.
```

Three rules make a working client: read `ready` first, answer both kinds of prompt, and ignore
what you do not recognize.

## Join a running session

A session open in the interface can be joined while you keep using it, and so can one a `luxe --rpc`
client started. `luxe join` connects to it and speaks this protocol on stdin and stdout, so the
client above works with `["luxe", "join"]` in place of `["luxe", "--rpc"]`.

```bash
luxe join --list    # sessions running on this machine
luxe join           # the one running in this folder
luxe join 5898d57b  # one by the start of its id, or by its Luxe's process id
```

After `ready` comes the session so far: `session_loaded` with the transcript, the latest
`state_changed`, `session_usage` and `workspace_updated`, a `tool_activity_updated` for each call
of a turn in flight, and any `permission_requested` or `question_asked` still waiting. Then the
live events, the same ones the interface gets.

A joined client is a guest in someone else's session:

- **Either side can answer a prompt.** The first answer counts, and `prompt_closed` tells everyone
  to put the prompt away. The interface says which client answered.
- **It can't switch or end the session.** `new_session` and `load_session` are refused with
  `action_rejected`. `close`, or the end of your stdin, disconnects you and leaves Luxe running.
- **GitHub writes are off**, and `ready` says `"github_writes": false`.
- **Keep reading.** A client that falls far behind gets a `notice` and is disconnected. Join
  again for a fresh copy of the session.

The interface lists who's joined in its dock, and says in the conversation when a client joins or
leaves, by the name it gives in `introduce` if it sends one within a second of connecting, or by
its process id. A device the bridge introduces as `yours`, paired by you, is listed but not
announced: you know your own phone joined, and a phone reconnects often.

A running Luxe lists its session in a file in its [runtime folder](environment.md#integrations),
`<pid>.json`, with the `address` to connect to: a Unix socket, or a named pipe on Windows. Connect
there yourself to skip `luxe join`.
To stop sessions being joined, set [`joinable`](configuration.md#joining-a-session) to `false`.

## Actions

| `kind` | `content` | Does |
| --- | --- | --- |
| `submit_user_message` | `{"text": "…", "images": [{"data": "<base64>", "mime": "image/png", "name": null}]}` | Start a turn. `images` is optional. |
| `steer_turn` | `{"text": "…"}` | Reach the running turn between tool calls, without ending it. With no turn running, starts one. |
| `withdraw_steering` | | Drop steering messages not yet delivered |
| `cancel` | | Stop the turn |
| `edit_resend` | `{"text": "…"}` | Replace the last message and send it again |
| `retry` | | Send the last message again |
| `regenerate` | | Replace the last answer |
| `respond_permission` | `{"id": "perm:…", "reply": "once"}` | Answer a permission prompt: `once`, `always`, `always_persist` or `reject` |
| `answer_question` | `{"id": "ask:…", "reply": {"answered": ["postgres"]}}` | Answer a question, or `{"reply": "cancelled"}` |
| `set_permission_auto_approve` | `true` | Approve every prompt for the rest of the session. Deny rules still apply. |
| `select_model` | `{"provider": "anthropic", "model": "claude-sonnet-5"}` | Switch models |
| `list_models` | `{"request_id": "…"}` | The models you can switch to, configured ones first, each with the thinking levels it honors; answered by `models_listed`, to you alone |
| `set_thinking_level` | `"high"` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh` or `ultra` |
| `set_mode` | `"plan"` | `build` or `plan`; see [Plan, then build](agent.md#plan-then-build). A name that is not a mode is answered by a `notice`. |
| `request_review` | `{"focus": "error handling"}`, or nothing | A [second opinion](agent.md#a-second-opinion) on the uncommitted changes; answered by `review_started`, then `review_finished` |
| `new_session` | | Start a new session |
| `load_session` | `"<session id>"` | Resume a session; answered by `session_loaded` |
| `refresh_sessions` | | Answered by `sessions_refreshed` |
| `refresh_workspace` | | Answered by `workspace_updated` |
| `request_context_composition` | | What fills the context window; answered by `context_composition` |
| `compact_now` | `{"focus": "the API decisions"}`, or nothing | Summarize now; answered by `compaction_started`, then `compaction_done` or `compaction_stopped` |
| `load_guided_review` | `{"request_id": "…", "target": {"kind": "working_tree"}}` | Capture a Guided Review; see [below](#guided-review) |
| `load_file_diff` | `{"request_id": "…", "path": "src/main.rs"}` | One file's change, outside any review; answered by `file_diff_loaded` |
| `list_directory` | `{"request_id": "…", "path": "src"}` | One project folder's entries, without what `.gitignore` ignores unless `"include_ignored": true`, which marks each such entry `"ignored": true`; answered by `directory_listed` |
| `read_project_file` | `{"request_id": "…", "path": "src/main.rs"}` | One project file's text, up to 2 MB; answered by `project_file_read` |
| `introduce` | `{"name": "iPhone"}` | A joined client's name, which the interface uses when it says who joined or answered. Add `"watch_only": true` as your first line to read without being announced; Luxe then drops everything else you send. `"yours": true` says the person at the terminal paired it, so it's listed without a line in the conversation. Refused from the client that started the session. |
| `close` | | Shut down, stopping a turn in flight the way `cancel` does. The stream ends after it. |

### Guided Review

A review is loaded with `load_guided_review`. Luxe answers with `guided_review_loaded`, which
carries the whole review (its `id`, `revision_id`, files, hunks and chapters), and then always
`review_session_restored`, which carries its comments. Everything below names the review by its
`id`. An action for a review that is no longer the open one is refused.

A session has one open review, and every load makes a new one, so a second client that loads its
own replaces the review the first is reading. To join it instead, send `show_open_review`.

```text
←  {"kind":"load_guided_review","content":{"request_id":"r1","target":{"kind":"working_tree"}}}
→  {"kind":"guided_review_loaded","content":{"request_id":"r1","review":{"id":"review:…","revision_id":"…",…}}}
→  {"kind":"review_session_restored","content":{"session":{"id":"review:…","findings":[],…}}}
←  {"kind":"edit_review","content":{"request_id":"e1","review_id":"review:…","edit":"comment","anchor":{"kind":"line","file_id":"file:…","hunk_id":"hunk:…","side":"new","line":12},"body":"Why retry twice?","severity":"question","send":true}}
→  {"kind":"review_session_saved","content":{"request_id":"e1","session":{…}}}
```

`edit_review` changes the comments. Every one is answered once, under its `request_id`: with
`review_session_saved` and the stored comments, or `review_session_save_failed` and why. The change
goes in `edit`, beside `request_id` and `review_id`:

| `edit` | Fields | Does |
| --- | --- | --- |
| `comment` | `anchor`, `body`; optionally `severity` (`issue` or `question`), `title`, `proposed_fix`, `send` | A new comment. `anchor` is `{"kind":"line","file_id","hunk_id","side","line"}`, a `hunk`, a `chapter`, or `{"kind":"review"}`. The title defaults to the first line. |
| `reply` | `finding_id`, `body`, optionally `send` | A reply on its thread. A resolved comment opens again. |
| `ask_for_fix` | `finding_id`, `body`, optionally `send` | Turn an answered question into a request for the fix |
| `edit_comment` | `finding_id`, `severity`, `title`, `body`, optionally `proposed_fix` | Rewrite a comment not sent yet |
| `delete_comment` | `finding_id` | Delete a comment not sent yet |
| `resolve`, `reopen` | `finding_id` | Mark a comment resolved, or open it again |
| `reanchor` | `finding_id`, `anchor` | Move a comment whose code a refresh moved away |
| `set_outcome` | `outcome`: `accepted`, `changes_requested` or `commented` | Record how the review ended. The agent is not told. |
| `send` | `finding_ids`; optionally `mode` (`request_fixes`, `comment_only`, `question`), `outcome` | Send comments to the agent at the next turn. The mode defaults to what the strongest comment asks for. |
| `cancel_send`, `retry_send` | `submission_id` | Withdraw a send the agent has not started, or send a failed one again |

`send` on a comment, reply or fix request sends it in the same save. A sent comment's turn shows up
as `review_session_saved` while it runs: the agent's answer to a question, and its report on a fix,
are recorded on the comments. You can keep editing while it works.

Each comment's `thread` says who said what, by `author`: `reviewer`, `agent`, or `system` for Luxe's
own word. A fix turn that ends with no report on a comment leaves a `system` entry saying why, and
the comment stays `open`.

| `kind` | `content` | Does |
| --- | --- | --- |
| `edit_review` | `{"request_id", "review_id", "edit", …}` | Change the comments, as above |
| `record_review_progress` | `{"review_id", "walked_chapter_ids": [...]}` | Which chapters were read. Saved when it changed, never answered. |
| `record_review_seen` | `{"review_id"}` | Remember what the walked chapters covered, so the next review of the same target leads with what is new |
| `show_open_review` | `{"request_id"}` | Ask for the review open now, whoever opened it, answered by `open_review_shown` |
| `refresh_guided_review` | `{"request_id", "review_id", "revision_id"}` | Read the change again, keeping the comments, if the review is still at `revision_id`. Answered like a load. |
| `commit_review_chapters` | `{"request_id", "review_id", "revision_id", "chapter_ids", "squash"}` | A commit per chapter, or one with `squash`. All chapters when `chapter_ids` is left out. Never pushes. Answered by `review_committed`. |
| `prepare_pull_request` | `{"request_id", "review_id"}` | Draft the pull request this branch's review would open; answered by `pull_request_prepared` |
| `open_pull_request` | `{"request_id", "title", "body", "draft"}` | Open the draft `request_id` named, with your title and description. Commits the reviewed work first when the draft said so. **A GitHub write.** |
| `prepare_stack` | `{"request_id", "review_id"}` | Draft a pull request per chapter; answered by `stack_prepared` |
| `open_stack` | `{"request_id", "draft"}` | Open the drafted stack, bottom first. **A GitHub write.** |
| `prepare_pull_request_post` | `{"request_id", "review_id"}` | Read what posting your review of a pull request needs; answered by `pull_request_post_prepared` |
| `post_pull_request_review` | `{"request_id", "verdict", "session_revision"}` | Post it with `comment`, `approve` or `request_changes`. **A GitHub write.** |
| `prepare_thread_answers` | `{"request_id", "review_id"}` | Whether your fixes are pushed yet, on your own pull request; answered by `thread_answers_prepared` |
| `answer_threads` | `{"request_id", "review_id", "answers": [{"finding_id", "reply", "resolve"}]}` | Reply to and resolve its threads, each named by the comment it became. **A GitHub write.** |

The `open_*` and `post_*` actions confirm a preview by its `request_id`. Luxe keeps what it
prepared, so you send back only what a person can change: a title, a description, a verdict.
`post_pull_request_review` is refused if the comments changed after the `session_revision` you drew
the preview from, so what posts is what you showed. A failed open or post can be confirmed again;
a stack opens once.

### GitHub

These go through your own `gh`, as the interface's `/github` and `/pr` do. `repository` is
`owner/name`, and you leave it out for the checkout's own repository.

| `kind` | `content` | Does |
| --- | --- | --- |
| `list_github` | `{"sections": [{"name": "Mine", "query": "is:open is:pr author:@me repo:{repo}"}]}`, or nothing | Run `/github`'s searches. With no `sections`, runs the ones in your config. Answered by `github_listed`. |
| `search_github` | `{"request": 7, "section": {…}, "after": "<cursor>"}` | One search on its own, or its next page; answered by `github_searched` with your `request` |
| `read_github_overview` | `{"repository": "acme/app", "number": 142, "pull_request": true}` | A pull request's or issue's status; answered by `github_overview_read` |
| `read_github_conversation` | `{"number": 142, "pull_request": true}` | Everything said on it; answered by `github_conversation_read` |
| `draft_github_reply` | `{"request_id": "…", "number": 142, "pull_request": true, "target": {…}, "conversation": […]}` | The agent drafts a reply in a turn that changes nothing; answered by `github_reply_drafted`. `target` and `conversation` are what `github_conversation_read` gave you. |
| `check_pull_request_fix` | `{"number": 142}` | Whether it can be checked out here to fix; answered by `pull_request_fix_checked` |
| `plan_restack` | `{"request_id": "restack:…"}` | What restacking this branch's stack would do; answered by `restack_planned`. Luxe keeps the plan. |
| `check_worktree_removal` | | What the session's worktree has not committed; answered by `worktree_removal_checked` |
| `set_focus` | `{"focused": false}` | Whether your window has focus. Luxe checks GitHub less often while it does not. |
| `link_pull_request` | `{"number": 142, "url": "…"}` | Record that this session worked on that pull request |
| `post_github_comment` | `{"number": 142, "thread": "PRRT_…", "body": "Fixed."}` | Post a reply in a thread, or a comment without `thread`; answered by `github_comment_posted`. **A GitHub write.** |
| `change_pull_request` | `{"number": 142, "write": {"change": "mark_ready"}}` | `update_branch`, `mark_ready`, `merge`, `enable_auto_merge` or `disable_auto_merge`; answered by `pull_request_changed`. **A GitHub write.** |
| `restack` | `{"request_id": "restack:…"}` | Push the plan `plan_restack` made under that id, exactly as planned; answered by `restacked` or `restack_failed`. **A GitHub write.** |

You confirm a restack by its id, not by sending the plan back: Luxe pushes the plan it showed you. A
newer `plan_restack` replaces the older plan, and a plan is pushed once. Before pushing, Luxe checks
that every moving branch here is where the plan saw it, checked out where it was, and clean, and
refuses with `restack_failed` otherwise. A push that did not finish is also `restack_failed`, and
its message says to check the branches on GitHub, since some may have moved. If the push succeeds
but the branches here, or a pull request's base on GitHub, cannot follow, the answer is `restacked`,
with what went wrong in `retarget_failed`. A `restack_planned` with nothing to restack, such as no
open pull request or no `gh`, carries a `refusal` and no `plan`.

`--issue`, `--from-pr` and `--worktree` work with `--rpc` as they do anywhere, since they decide the
session before the first frame. See [Git and GitHub](github.md).

An unknown `kind` is answered with `action_rejected`, which is also how to find out what a build
supports. So is a malformed line, and nothing else happens: one bad line never ends the session.

```text
←  {"kind":"steer_turn","content":{}}
→  {"kind":"action_rejected","content":{"kind":"steer_turn","reason":"steer_turn: missing field `text`"}}
```

## Events

There are around sixty. These are the ones a front end is built on:

| `kind` | Is |
| --- | --- |
| `ready` | The handshake: protocol version, Luxe version, session ID, directory |
| `token_delta`, `thinking_delta` | The reply and reasoning as they stream. `block` orders them. |
| `tool_call_composing` | The model has started writing a tool call. Optional; useful for showing a `write` filling in. |
| `tool_started`, `tool_progress`, `tool_completed`, `tool_ended` | A tool call's progress, by `call_id`. `tool_completed` carries its output. |
| `tool_activity_updated` | The card the interface draws for a call: title, body, state and timing |
| `message_committed` | A complete message was added to the transcript |
| `permission_requested` | **Answer it** with `respond_permission`, or the turn waits. If `always_patterns` is empty, offer only `once` and `reject`. |
| `question_asked` | **Answer it** with `answer_question` |
| `turn_started` | A turn began, from any client. It comes before the turn's message is saved, and `turn_ended` always follows. A client joining mid-turn gets it as part of catching up. |
| `turn_ended` | The turn finished; `reason` says why |
| `provider_retrying` | The provider failed before answering, and Luxe is trying again |
| `state_changed` | Model, thinking level, mode, and whether a turn is running |
| `plan_written` | Plan mode wrote `path`. The interface offers the plan's next step when the turn ends. |
| `review_started`, `review_finished` | A requested review began on `model`, and came back with a `verdict` (`continue`, `fix` or `unavailable`) and `findings`. On `fix` the findings also become the agent's next turn. |
| `turn_usage`, `session_usage`, `plan_usage` | Cost per call, for the session, and what a subscription has left |
| `context_usage`, `context_composition` | How full the context window is, and with what |
| `compaction_started`, `compaction_done`, `compaction_stopped` | A summary of the conversation is being written, on request or mid-turn. It can take a minute. It always ends in `compaction_done` (with `tokens_after`) or `compaction_stopped` (failed or cancelled). |
| `workspace_updated` | The project, Git changes, todos and tools: what the dock shows |
| `prompt_closed` | A prompt stopped waiting, answered here or by another client, or cancelled. Take it down by `id`. |
| `session_loaded`, `sessions_refreshed` | A transcript was restored; the session list changed |
| `notice`, `error` | Something to show. `error` ends the turn. |
| `guests_changed` | The names of the clients joined now, sent to the interface as one joins or leaves, for its dock. |
| `action_rejected` | Your frame was refused and nothing happened |

Guided Review, as the interface sees it:

| `kind` | Is |
| --- | --- |
| `guided_review_loading`, `guided_review_loaded`, `guided_review_failed` | A review you asked for, by `request_id`. `loaded` carries the whole review: files, hunks and chapters. |
| `guided_review_guide_updated` | Better chapters and a summary replaced the first ones, for the same pinned revision |
| `review_session_restored` | The comments and progress that go with the review just loaded. It always follows `guided_review_loaded`, after a restart too. |
| `open_review_shown` | The answer to `show_open_review`, under its `request_id`: the open `review` and its `session`, or neither when no review is open. `reading` says what each chapter asks of the reader: `needs_review` items, and the `reason` when one thing accounts for all of them. |
| `review_session_saved`, `review_session_save_failed` | A change to them was stored, or refused. Luxe reports its own changes this way too: a turn starting, the agent's report, a review posted. |
| `review_feedback_turn_dispatched`, `review_feedback_turn_completed`, `review_feedback_turn_failed` | Feedback you sent reaching the agent, and how its turn went |
| `review_questions_answered`, `remediation_reported` | The agent's answer to a question, and its report on a fix |
| `review_committed`, `review_commit_failed` | Committing a review's chapters |
| `last_turn_available` | The agent's last turn can be reviewed on its own |
| `file_diff_loaded`, `file_diff_failed` | One file's change, for `load_file_diff` |
| `directory_listed`, `directory_list_failed` | One folder's entries, for `list_directory` |
| `models_listed` | The models for `list_models`: each has `provider` and `model` for `select_model`, a `name` to show, and its `thinking_levels` |
| `project_file_read`, `project_file_read_failed` | One file's text, for `read_project_file` |

GitHub:

| `kind` | Is |
| --- | --- |
| `github_listed`, `github_searched` | Search results, with each section's rows or why it has none |
| `github_overview_read`, `github_conversation_read` | One pull request or issue: its status, or everything said on it |
| `github_reply_drafted`, `github_comment_posted` | A draft to edit and send, and where a sent one was posted |
| `pull_request_changed`, `pull_request_fix_checked` | What a change did, and whether a fix can be checked out here |
| `restack_planned`, `restacked`, `restack_failed` | A restack's plan, and what pushing it did |
| `pull_request_prepared`, `pull_request_opened`, `pull_request_open_failed` | Opening a pull request from a review |
| `stack_prepared`, `stack_opened` | Opening a review as a stack |
| `pull_request_post_prepared`, `pull_request_review_posted`, `pull_request_post_failed` | Posting your review of a pull request |
| `thread_answers_prepared`, `threads_answered`, `thread_answers_failed` | Replying to and resolving review threads on your own pull request |
| `worktree_removal_checked` | What removing the session's worktree would lose |

The branch's pull request, its checks and its review status come in `workspace_updated`, as they
do for the interface's dock.

Treat an event you do not recognize as nothing.

## Versioning

`protocol_version` is `2`. New events, actions and fields do not change it, since a client that
ignores what it does not know already handles them. It changes only when something a client could
have relied on is removed or renamed. So dispatch on `kind`, ignore the rest, and do not assume the
list is closed.

## Security

- Nothing listens on the network. A [joined](#join-a-running-session) session takes connections
  on a local socket or pipe that only your user account can open, and turns away any program the
  agent itself started, so the agent can't answer its own prompts.
- Permission rules are enforced inside Luxe. A client cannot weaken a `deny`: reading `.env` is
  refused here as everywhere.
- Prompts go to your client to answer. To approve everything, send `set_permission_auto_approve` or
  start with `--auto-approve`. `--on-ask` is refused, since your client can answer.
- `--permission-mode`, `--read-only`, `--yolo` and `--trust-project` apply as usual.

### GitHub writes

Your client can read GitHub without asking. To let it publish there (post a comment or a review,
answer threads, open, change, merge or restack a pull request), start Luxe with `--github-writes`.
Without it, those actions are refused with `action_rejected`, and `ready` says
`"github_writes": false` so you can hide them.

- **Read-only wins.** With `--read-only`, or a read-only mode in your permissions file, the writes
  are refused even with the flag, and so is `commit_review_chapters`.
- **It is a flag, not a setting.** Whoever starts Luxe decides, for that session. No config file
  turns it on, and a project cannot.
- **It covers your client, not the agent.** The agent's own GitHub writes are permission rules: they
  ask first, and the prompt comes to your client. See [Security](security.md#when-you-step-away).

Luxe is not a sandbox. To isolate it, run it in a [container](security.md#isolation).
