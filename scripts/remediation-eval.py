#!/usr/bin/env python3
"""Ask real models to fix review comments and see whether luxe can read what they report.

A fix request ends with a remediation report, and the review learns what the agent did only from
it. Whether a model writes one luxe can read is a property of the model and of the prompt, so it is
measured against live models rather than asserted in a unit test. Each run is a scratch repository
with two bugs, a working-tree review, a comment on each, and a fix request over `luxe --rpc`; the
turn's assistant text is then read back from the session log and sorted by how it wrapped its
report.

    scripts/remediation-eval.py --runs 3 --out /tmp/remediation github-copilot/gpt-4o github-copilot/gpt-5-mini

It costs real calls and its numbers move with the models, so it is a check to run after changing
the prompt or the reader, not a gate.
"""

import argparse
import concurrent.futures
import json
import os
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time

SOURCE_BEFORE = '''def total(numbers):
    result = 0
    for index in range(len(numbers)):
        result += numbers[index]
    return result


def average(numbers):
    return total(numbers) / len(numbers)
'''

SOURCE_AFTER = '''def total(numbers):
    result = 0
    for index in range(1, len(numbers)):
        result += numbers[index]
    return result


def average(numbers):
    return total(numbers) / len(numbers)
'''


class Luxe:
    def __init__(self, luxe, model, cwd):
        self.proc = subprocess.Popen(
            [luxe, "--rpc", "--model", model], cwd=cwd, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.frames = queue.Queue()
        self.kinds = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.proc.stdout:
            self.frames.put(json.loads(line))
        self.frames.put(None)

    def send(self, kind, content=None):
        frame = {"kind": kind} | ({"content": content} if content is not None else {})
        self.proc.stdin.write(json.dumps(frame) + "\n")
        self.proc.stdin.flush()

    def wait(self, kind, match=lambda c: True, timeout=400):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                frame = self.frames.get(timeout=max(0.1, deadline - time.time()))
            except queue.Empty:
                break
            if frame is None:
                raise RuntimeError(f"stream ended waiting for {kind}")
            k, c = frame["kind"], frame.get("content")
            self.kinds.append(k)
            if k == "permission_requested":
                self.send("respond_permission", {"id": c["id"], "reply": "once"})
                continue
            if k == "question_asked":
                self.send("answer_question", {"id": c["id"], "reply": "cancelled"})
                continue
            if k == "action_rejected":
                raise RuntimeError(f"rejected: {c}")
            if k == kind and match(c):
                return c
        raise RuntimeError(f"timed out waiting for {kind}")

    def close(self):
        try:
            self.send("close")
            self.proc.wait(timeout=60)
        except Exception:
            self.proc.kill()


def git(cwd, *args):
    subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True)


def scratch():
    root = tempfile.mkdtemp(prefix="remed-eval-")
    git(root, "init", "-q")
    git(root, "config", "user.email", "eval@example.invalid")
    git(root, "config", "user.name", "Eval")
    open(os.path.join(root, "stats.py"), "w").write(SOURCE_BEFORE)
    git(root, "add", ".")
    git(root, "commit", "-qm", "initial")
    open(os.path.join(root, "stats.py"), "w").write(SOURCE_AFTER)
    return root


def anchor(review, line_text):
    for file in review["files"]:
        for hunk in file["hunks"]:
            for line in hunk["lines"]:
                if line_text in line.get("text", line.get("content", "")) and line["kind"] == "addition":
                    return {"kind": "line", "file_id": file["id"], "hunk_id": hunk["id"],
                            "side": "new", "line": line["new_line"]}
    for file in review["files"]:
        return {"kind": "file", "file_id": file["id"]}


def assistant_texts(root):
    """Assistant text per message after the last fix request, in order."""
    sessions = os.path.join(root, ".luxe", "sessions")
    path = max((os.path.join(d, f) for d, _, fs in os.walk(sessions) for f in fs if f.endswith(".jsonl")),
               key=os.path.getmtime)
    texts, started = [], False
    for line in open(path):
        kind = json.loads(line)["kind"]
        if kind["kind"] != "message":
            continue
        message = kind["data"]
        text = "".join(b.get("content", "") for b in message["content"] if b.get("type") == "text")
        if message["role"] == "user" and "Mode: RequestFixes" in text:
            texts, started = [], True
            continue
        if started and message["role"] == "assistant":
            calls = sum(1 for b in message["content"] if b.get("type") == "tool_call")
            texts.append({"text": text, "tool_calls": calls})
    return texts


def user_texts(root):
    """Every user message in the newest session, as text."""
    sessions = os.path.join(root, ".luxe", "sessions")
    path = max((os.path.join(d, f) for d, _, fs in os.walk(sessions) for f in fs if f.endswith(".jsonl")),
               key=os.path.getmtime)
    texts = []
    for line in open(path):
        kind = json.loads(line)["kind"]
        if kind["kind"] == "message" and kind["data"]["role"] == "user":
            texts.append("".join(b.get("content", "") for b in kind["data"]["content"] if b.get("type") == "text"))
    return texts


def classify(texts):
    final = texts[-1]["text"] if texts else ""
    every = "\n".join(t["text"] for t in texts)
    fences = re.findall(r"^[ \t]*(?:```|~~~)([^\n`]*)$", final, re.M)
    has_json = '"findings"' in final
    earlier = any('"findings"' in t["text"] for t in texts[:-1])
    if "```luxe-remediation" in final:
        shape = "named fence"
    elif has_json and fences:
        shape = f"other fence ({','.join(sorted(set(f.strip() or 'bare' for f in fences)))})"
    elif has_json:
        shape = "unfenced json"
    elif earlier:
        shape = "report in an earlier message"
    else:
        shape = "no report"
    return shape, final, every


def run_one(luxe_bin, model, out, index):
    root = scratch()
    luxe = Luxe(luxe_bin, model, root)
    record = {"model": model, "run": index, "root": root}
    try:
        luxe.wait("ready")
        luxe.send("load_guided_review", {"request_id": "load", "target": {"kind": "working_tree"}})
        review = luxe.wait("guided_review_loaded", lambda c: c["request_id"] == "load")["review"]
        rid = review["id"]
        luxe.send("edit_review", {"request_id": "c1", "review_id": rid, "edit": "comment",
                                  "anchor": anchor(review, "range(1"), "severity": "blocking",
                                  "body": "This skips the first number, so every total is wrong."})
        luxe.wait("review_session_saved", lambda c: c["request_id"] == "c1")
        luxe.send("edit_review", {"request_id": "c2", "review_id": rid, "edit": "comment",
                                  "anchor": anchor(review, "range(1"), "severity": "suggestion",
                                  "body": "average() divides by zero on an empty list; return 0.0 there."})
        saved = luxe.wait("review_session_saved", lambda c: c["request_id"] == "c2")["session"]
        ids = [f["id"] for f in saved["findings"]]
        luxe.send("edit_review", {"request_id": "send", "review_id": rid, "edit": "send",
                                  "finding_ids": ids, "mode": "request_fixes"})
        reported = {"report": None}

        def done(c):
            return any(s["dispatch"] in ("completed", "failed", "failed_retryable") and s["mode"] == "request_fixes"
                       for s in c["session"]["submissions"])
        started = time.time()
        while True:
            frame = luxe.wait("review_session_saved", timeout=600)
            if done(frame):
                break
        # the report event precedes completion, so it is in the kinds seen
        record["reported"] = "remediation_reported" in luxe.kinds
        record["asked"] = any(t.startswith("Guided review report") for t in user_texts(root))
        record["seconds"] = round(time.time() - started)
        threads = {f["label"]: [m["body"] for m in f["thread"] if m["author"] == "agent"]
                   for f in frame["session"]["findings"]}
        record["agent_notes"] = threads
        record["states"] = {f["label"]: f["state"] for f in frame["session"]["findings"]}
    except Exception as error:
        record["error"] = str(error)
    finally:
        luxe.close()
    try:
        texts = assistant_texts(root)
        record["shape"], final, every = classify(texts)
        record["messages"] = len(texts)
        name = f"{model.replace('/', '_')}-{index}"
        open(os.path.join(out, name + ".final.txt"), "w").write(final)
        open(os.path.join(out, name + ".all.json"), "w").write(json.dumps(texts, indent=1))
    except Exception as error:
        record.setdefault("error", f"reading transcript: {error}")
    open(os.path.join(out, "results.jsonl"), "a").write(json.dumps(record) + "\n")
    print(f"{model:40} run {index}: reported={record.get('reported')} asked={record.get('asked')} "
          f"shape={record.get('shape')} {record.get('error', '')}", flush=True)
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--luxe", default="luxe", help="the binary to drive; target/debug/luxe for a branch")
    parser.add_argument("--runs", type=int, default=2)
    parser.add_argument("--out", required=True)
    parser.add_argument("--parallel", type=int, default=4)
    parser.add_argument("models", nargs="+")
    args = parser.parse_args()
    # luxe runs inside each scratch repository, so a relative path has to be made absolute first
    found = shutil.which(args.luxe) if os.sep not in args.luxe else os.path.abspath(args.luxe)
    if not found or not os.access(found, os.X_OK):
        parser.error(f"no luxe at {args.luxe}")
    args.luxe = found
    os.makedirs(args.out, exist_ok=True)
    jobs = [(m, i) for i in range(args.runs) for m in args.models]
    with concurrent.futures.ThreadPoolExecutor(args.parallel) as pool:
        records = list(pool.map(lambda job: run_one(args.luxe, job[0], args.out, job[1]), jobs))
    print(f"\n{'model':40} {'read':>6} {'asked':>6} {'runs':>6}")
    for model in args.models:
        mine = [r for r in records if r["model"] == model]
        read = sum(1 for r in mine if r.get("reported"))
        asked = sum(1 for r in mine if r.get("asked"))
        print(f"{model:40} {read:>6} {asked:>6} {len(mine):>6}")
    print(f"\ntranscripts and results.jsonl in {args.out}")


if __name__ == "__main__":
    main()
