# actions demo — the send-back loop, driven through the pane

From the **repo root** (the fixture's action templates are
repo-root-relative, so the pane must be started here), after a build and
with `target/release` on PATH (see the root README's install section):

```sh
cargo build --release --locked && export PATH="$PWD/target/release:$PATH"
demos/actions/reset.sh                        # re-date the fixture to now
export DAGR_RUN=$PWD/demos/actions/run.json
dagr view
```

`reset.sh` rewrites the committed fixture in place, so `git status`
will show `demos/actions/run.json` modified afterwards. When you're
done with the demo: `git checkout -- demos/actions/run.json`.

`reset.sh` matters: the producer stamps real wall-clock times, and
running the demo against the fixture's committed dates would make every
age and ETA read as months (the analytics use the document's own clock,
which would then straddle the gap).

`u` on the blocked question, `x` on the review (type a reason at the
prompt) — each shows the exact producer argv at a confirm gate, runs it,
and renders whatever `producer.py` writes back: the directive event, the
review settled `done` with its verdict as receipt, the rejected `L1·a1`,
and the new `L1·a2` attempt with its `sent_back` cause (the ↩ re-entry).
Note the reject argv carries `--reopen L1` **literally in the template**:
the task a send-back reopens is disclosed at the confirm gate, never
resolved inside the CLI where the gate can't show it. And the review's
own attempt settles `done` — the review succeeded at producing a verdict;
what the verdict rejects is the attempt under review. dagr changes no
state itself; `producer.py` is the reference producer CLI.

What the reference producer actually guarantees (worth copying):

- **one intent, applied once** — used idempotency keys live *inside*
  the run document (`x_producer.applied_keys`; the contract ignores
  unknown fields), so key and state commit in the same atomic rename.
  A crash either applied nothing or recorded the key with the state it
  paid for; replaying a key is a no-op success.
- **no silent retargeting** — accept/reject settle exactly the attempt
  named at the confirm gate. A superseded or already-settled attempt is
  an error telling you what changed, never a quiet swap to "latest".
- **validate, then publish** — every candidate passes
  `dagr check --strict` before the rename; an invalid candidate is
  discarded, and the projection rules (E150) are applied per verb.
- **serialized** — a pid-stamped lock file covers the whole
  read→mutate→validate→publish span; a dead holder's lock is broken.

Trust boundary reminder for template authors: dagr executes the argv
directly, no shell — but that only holds if you keep it that way. Never
write templates like `["sh", "-c", "… {text} …"]`; inside an interpreter
string, typed text is code (CONTRACT §9).

`receipts/` holds the evidence: `01-cli-transcript.txt` (every refusal
path, the replay no-op, the send-back shape, the final clean check and
sane analytics), `02-after-cli.json`, `03-pane-e2e.txt` (a live
herdr-pane session: confirm-gate argv with key, producer run, watcher
reload, the ↩ re-entry rendering), and `04-after-pane.json` — both
after-documents strict-clean with the key ledger inside.
