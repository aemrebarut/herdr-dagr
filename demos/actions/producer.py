#!/usr/bin/env python3
"""Reference producer CLI for the §9 actions demo.

This is what a real orchestrator's CLI looks like from dagr's side: a
command that owns the run file, dedupes on idempotency keys, and writes
contract-valid shapes (directive events, new attempts with causes) that
the pane then renders. dagr itself never touches the file.

  producer.py unblock <task> --by <who> --key <k>
  producer.py answer  <task> --text <t> --by <who> --key <k>
  producer.py accept  <task> --attempt <a> --by <who> --key <k>
  producer.py reject  <task> --attempt <a> --reason <r> [--reopen <t>] --by <who> --key <k>

The run file is resolved from $DAGR_RUN (same variable dagr view uses).

Reject with --reopen is a SEND-BACK: the review attempt settles `done`
(the review succeeded — its verdict is its receipt), what gets rejected
is the reopened task's latest attempt, and a fix round opens there with
a `sent_back` cause. Recording the review itself as rejected is the
review-state inversion; don't copy it (see the producer skill). The
reopen target is an explicit flag so it appears IN the argv the human
approves at the confirm gate — never resolved from `deps` inside the
CLI, where the gate can't show it. Without --reopen, reject is a plain
rejection of the named attempt.

Transaction discipline (the part worth copying):
  - one lock file serializes read → mutate → validate → publish
  - used keys live INSIDE the run document (x_producer.applied_keys —
    the contract ignores unknown fields), so key and state commit in
    the same atomic os.replace: a crash either applied nothing or
    recorded the key with the state it paid for. No two-file ordering.
  - the candidate is validated with `dagr check --strict` BEFORE the
    rename; an invalid candidate is discarded, never published
  - every action names its exact target attempt and expected pre-state;
    a stale target (superseded or already settled) is an error, not a
    silent retarget

Known simplification: the applied-key ledger grows monotonically with
operator activity. A real producer should compact entries whose
referenced attempts are all terminal; this demo keeps every key.
"""

import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone

TERMINAL = {"done", "failed", "rejected", "settled_unverified", "lost"}


def now() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def dagr_bin() -> str:
    """Same resolution order as the producer skill's preflight."""
    if os.environ.get("DAGR_BIN"):
        return os.environ["DAGR_BIN"]
    from shutil import which

    if which("dagr"):
        return "dagr"
    here = os.path.dirname(os.path.abspath(__file__))
    for profile in ("release", "debug"):
        cand = os.path.join(here, "..", "..", "target", profile, "dagr")
        if os.path.exists(cand):
            return cand
    sys.exit("producer: no validator (set DAGR_BIN, or `cargo build`)")


class Lock:
    """pid-stamped lock file; a dead holder's lock is broken once."""

    def __init__(self, path):
        self.lockfile = path + ".lock"

    def __enter__(self):
        deadline = time.monotonic() + 5.0
        broke_stale = False
        while True:
            try:
                fd = os.open(self.lockfile, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
                os.write(fd, str(os.getpid()).encode())
                os.close(fd)
                return self
            except FileExistsError:
                try:
                    pid = int(open(self.lockfile).read().strip() or "0")
                except (OSError, ValueError):
                    pid = 0
                alive = False
                if pid:
                    try:
                        os.kill(pid, 0)
                        alive = True
                    except (ProcessLookupError, PermissionError) as e:
                        alive = isinstance(e, PermissionError)
                if not alive and not broke_stale:
                    broke_stale = True  # recover an interrupted run, once
                    try:
                        os.unlink(self.lockfile)
                    except FileNotFoundError:
                        pass
                    continue
                if time.monotonic() > deadline:
                    sys.exit(f"producer: {self.lockfile} held by pid {pid}")
                time.sleep(0.1)

    def __exit__(self, *exc):
        try:
            os.unlink(self.lockfile)
        except FileNotFoundError:
            pass


def publish(path, doc, key):
    """Validate the candidate, then atomically commit state + key."""
    doc["generated_at"] = now()
    if key:
        ledger = doc.setdefault("x_producer", {}).setdefault("applied_keys", [])
        ledger.append(key)
    tmp = f"{path}.{os.getpid()}.tmp"  # unique: concurrent losers can't collide
    with open(tmp, "w") as fh:
        json.dump(doc, fh, indent=2, ensure_ascii=False)
    chk = subprocess.run(
        [dagr_bin(), "check", tmp, "--strict", "--json"],
        capture_output=True,
        text=True,
    )
    if chk.returncode != 0 or chk.stdout.strip() != "[]":
        os.unlink(tmp)
        sys.exit(
            "producer: candidate failed `dagr check --strict` — not published:\n"
            + (chk.stdout.strip() or chk.stderr.strip())
        )
    os.replace(tmp, path)


def find_task(doc, tid):
    for t in doc.get("tasks", []):
        if t.get("id") == tid:
            return t
    sys.exit(f"producer: unknown task {tid!r}")


def latest_attempt(task):
    atts = task.get("attempts", [])
    return max(atts, key=lambda a: a.get("n", 0)) if atts else None


def resolve_attempt(task, tid, aid):
    """The attempt the operator SAW, verified fresh and still open."""
    if not aid:
        sys.exit(f"producer: --attempt is required (name what you confirmed)")
    att = next((a for a in task.get("attempts", []) if a.get("id") == aid), None)
    if att is None:
        sys.exit(f"producer: {tid} has no attempt {aid!r}")
    last = latest_attempt(task)
    if last is not att:
        sys.exit(
            f"producer: {aid} is stale — {last.get('id')} superseded it; "
            "re-open the run and confirm against the current attempt"
        )
    if att.get("state") in TERMINAL:
        sys.exit(f"producer: {aid} is already {att.get('state')} — nothing to settle")
    return att


def project_task_state(task):
    """CONTRACT's projection table (E150): live task state follows the
    latest attempt; blocked is only for tasks that still carry a question."""
    last = latest_attempt(task)
    if last and last.get("state") == "working":
        task["state"] = "working"
    else:
        task["state"] = "queued"


def event(doc, **kw):
    doc.setdefault("events", []).append({"at": now(), **kw})


def main():
    args = sys.argv[1:]
    if not args:
        sys.exit(__doc__)
    verb, pos, opts = args[0], [], {}
    it = iter(args[1:])
    for a in it:
        if a.startswith("--"):
            opts[a[2:]] = next(it, "")
        else:
            pos.append(a)
    path = os.environ.get("DAGR_RUN")
    if not path:
        sys.exit("producer: set DAGR_RUN to the run file")
    key, by = opts.get("key", ""), opts.get("by", "operator")

    with Lock(path):
        with open(path) as fh:
            doc = json.load(fh)
        # idempotency: same key twice = same intent, applied once. The
        # ledger travels in the document, so it can never disagree with it.
        seen = doc.get("x_producer", {}).get("applied_keys", [])
        if key and key in seen:
            print(f"{verb}: key {key} already applied (idempotent no-op)")
            return
        tid = pos[0] if pos else sys.exit("producer: missing task id")
        task = find_task(doc, tid)

        if verb == "unblock":
            if task.get("state") != "blocked":
                sys.exit(f"producer: {tid} is not blocked")
            event(doc, type="directive", verb="unblock", by=by, task=tid,
                  detail=f"unblocked by {by}")
            task.pop("unblock", None)
            project_task_state(task)
        elif verb == "answer":
            if task.get("state") != "blocked":
                sys.exit(f"producer: {tid} is not blocked — nothing to answer")
            text = opts.get("text", "")
            event(doc, type="directive", verb="answer", by=by, task=tid,
                  detail=text)
            task.pop("unblock", None)
            project_task_state(task)
        elif verb == "accept":
            att = resolve_attempt(task, tid, opts.get("attempt"))
            att["state"] = "done"
            att["ended_at"] = att.get("ended_at") or now()
            att["outcome"] = {"result": "done", "evidence": "reported",
                              "receipt": f"accepted by {by}"}
            task["state"] = "done"
            event(doc, type="attempt_settled", task=tid, attempt=att["id"],
                  actor=by, detail=f"accepted by {by}")
        elif verb == "reject":
            reason = opts.get("reason", "rejected")
            att = resolve_attempt(task, tid, opts.get("attempt"))
            reopen = opts.get("reopen")
            event(doc, type="directive", verb="reject", by=by, task=tid,
                  detail=reason)
            if reopen:
                # send-back: the review SUCCEEDED — it produced the verdict
                # that opens the fix round, so the review attempt settles
                # done with the verdict as its receipt. What gets rejected
                # is the attempt under review; recording the review itself
                # as rejected would be the review-state inversion.
                att["state"] = "done"
                att["ended_at"] = att.get("ended_at") or now()
                att["outcome"] = {"result": "done", "evidence": "reported",
                                  "receipt": f"verdict: changes-required — {reason}"}
                task["state"] = "done"
                event(doc, type="attempt_settled", task=tid, attempt=att["id"],
                      actor=by, detail=f"verdict: changes-required — {reason}")
                impl = find_task(doc, reopen)
                last_impl = latest_attempt(impl)
                if last_impl and last_impl.get("state") not in ("rejected", "failed"):
                    last_impl["state"] = "rejected"
                    last_impl["ended_at"] = last_impl.get("ended_at") or now()
                    last_impl["outcome"] = {
                        "result": "rejected", "evidence": "reported",
                        "reason": reason,
                        "receipt": f"review verdict {att['id']}: changes-required"}
                n = (last_impl or {}).get("n", 0) + 1
                impl.setdefault("attempts", []).append({
                    "id": f"{reopen}·a{n}", "n": n,
                    "cause": {"type": "sent_back", "by": by,
                              "ref": att["id"], "reason": reason},
                    "actor": impl.get("owner", "dev"), "state": "queued",
                })
                impl["state"] = "queued"
                event(doc, type="attempt_started", task=reopen,
                      attempt=f"{reopen}·a{n}", actor=impl.get("owner", "dev"),
                      detail=f"fix round opened by verdict {att['id']}")
            else:
                # plain rejection of the named attempt; no fix round
                att["state"] = "rejected"
                att["ended_at"] = att.get("ended_at") or now()
                att["outcome"] = {"result": "rejected", "evidence": "reported",
                                  "reason": reason}
                task["state"] = "rejected"
        else:
            sys.exit(f"producer: unknown verb {verb!r}")

        publish(path, doc, key)
    print(f"{verb} {tid} ✓")


if __name__ == "__main__":
    main()
