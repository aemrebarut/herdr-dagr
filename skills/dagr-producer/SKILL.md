---
name: dagr-producer
description: Emit and maintain a dagr run file — a live, contract-valid JSON description of recursive projects, tasks, attempts, gates, evidence, policies, events, and operator-message resolutions that `dagr view` renders as a DAG. Use when orchestrating agents or tracking multi-step work that a dagr pane should display.
---

# dagr producer — write the run, prove the run

You are the **producer**: the single writer of a run file that `dagr view`
renders live. dagr is a *representation kernel*: you assert task truth, and
it derives only defined view signals from those facts. Missing or wrong facts
still produce a missing or wrong graph; there is no workflow engine to repair it.

Contract version: **`"dagr": 3`** (v1/v2 files remain readable; write v3 for new runs).

## Find your validator (before you write anything)

The check loop below is the only feedback you get, so resolve the `dagr`
binary FIRST and stop if you can't:

1. `$DAGR_BIN` if set;
2. `command -v dagr` (on PATH);
3. the plugin/repo build: `<dagr repo>/target/release/dagr` — inside a
   herdr pane the runtime injects `$HERDR_PLUGIN_ROOT`, which IS the dagr
   repo root;
4. fallback: `cargo run --manifest-path <dagr repo>/Cargo.toml -- check …`.

No validator available → do **not** start writing run files; say so and
stop. An unvalidated run file is exactly the silent wrongness this whole
system exists to prevent.

## The loop (non-negotiable)

```
write run.json.tmp → dagr check run.json.tmp --strict --json → fix → repeat until []
                                                             → THEN rename over run.json
```

**Validate the candidate, then publish it — never the other way around.**
The pane renders whatever `run.json` holds, immediately; renaming an
invalid candidate over it shows your error state to every viewer while
you iterate. The safe transaction:

1. Write the complete next document to `run.json.tmp` (same directory —
   rename must be atomic, so same filesystem).
2. `dagr check run.json.tmp --strict --json`. Exit 0 with `[]` is clean;
   exit 1 means findings (E-codes are errors; W-codes mean representable-
   but-suspect — fix them unless you can say why not; `--strict` treats
   them as failures, prefer it). **Exit 2 means the validator could not
   read the file at all** — a path or tooling problem, never a document
   problem; stdout is empty on that path, so never treat empty output as
   clean. Stop, re-run the preflight, and never publish on a non-zero
   exit.
3. Only when clean: `mv run.json.tmp run.json` (atomic rename — the pane
   reloads on mtime and never sees a half-written or invalid file).
4. On failure: fix the temp file and re-check. The previous **live** file
   stays untouched — never leave `run.json` itself in an error state.

**Where to write it**: the pane looks for `$DAGR_RUN` first, then
`.dagr/run.json`, then `run.json` under the workspace cwd — and waits on
`.dagr/run.json` when none exists yet. Default to `.dagr/run.json` in
the workspace root (gitignore it), and one run per file: parallel lanes
inside a run are just tasks with disjoint `deps`, but a separate
workflow gets its own run file and its own pane.

## Object model in one breath

- **Project** = the recursive visual scope. The run is the implicit root;
  `projects[]` may have `parent`. A phase or workstream is just a project,
  not another entity. Each task has one optional `project` home, while its
  dependency edges may cross any project boundary.
- **Task** = the work item. Stable `id` you choose (never a pane id).
  `kind` is an open set — `impl · review · test · gate · question · docs ·
  ship · …` — pick the honest one (`question` for a task that exists to be
  answered by a human, `gate` for fan-ins; both change how dagr draws it).
  States: `queued · working · review · blocked · done · failed · rejected ·
  canceled · settled_unverified`.
- **Attempt** = one try at a task. `id` styled `T·aN`, 1-based `n`.
  States: `queued · working · done · failed · rejected ·
  settled_unverified · lost`. **A retry never rewrites an attempt — it
  appends a new one with a `cause`.** A task's *history* never moves
  backward — you never rewrite or delete an attempt. A task's *state*
  follows its latest attempt, so a send-back moves a `done` task back to
  `working` as its new attempt opens.
- **Event** = append-only provenance: `attempt_started · attempt_settled ·
  promoted · directive · message_resolved · note`, ascending `at` timestamps. Never rewrite
  or reorder events.
- **Evidence tier** on every terminal outcome: `verified` (mechanically
  checked — test run, commit receipt) · `reported` (typed envelope from
  the actor) · `heuristic` (inferred) · `asserted` (bare claim). "The
  agent said done" is at best `reported`. Missing envelope? That's
  `settled_unverified` — a real terminal state, not a soft `done`.

## Invariants dagr check will hold you to

1. **Task state is a projection over attempts**: `working` needs a
   working attempt; `done`/`rejected`/`settled_unverified` need the latest
   attempt to match; `failed` accepts a latest attempt of `failed` or
   `lost`; `queued` forbids a working attempt, and forbids a latest
   attempt of `done` or `settled_unverified` — a task re-queued after a
   `failed` or `rejected` attempt is correct and expected; `review` needs
   at least one attempt. `canceled` is task-only: it withdraws planned work
   without rewriting or inventing an attempt.
2. **Causes point backward in time**: attempt n>1 carries
   `cause` (`sent_back · gate_failed · followup · superseded`) whose `ref`
   names an *earlier* attempt. `initial` only for n=1.
3. **The run is a DAG**: no cycles through `deps` or gate
   `inputs`. A gate's fan-in IS its `deps`; `inputs` exists only to
   override when the fan-in set differs from the dependency set.
   Encode true sequential work as dependencies; task declaration order is
   only the attempt-less sibling tiebreak.
4. **Terminal attempts carry `outcome`** with `result` == `state`
   and a real evidence tier; timestamps are real ISO-8601
   and attempts end after they start.
5. **Ids are yours and unique**: task ids never collide
   with attempt ids. herdr pane ids go in `locator`, never in `id`.
6. **Live attempts are locatable and alive**: working attempts want a
   `locator` (`{"pane": "wX:pN"}`) and a populated `liveness`
   — `prompt_acknowledged` (bool), `last_output_at` (timestamp
   string), `queued_input` (**count** of composer lines typed but
   unsubmitted, `0` when none — a number, not a bool; a bool rejects the
   whole document). Update `last_output_at` when your agents
   produce output; staleness is rendered, silence is the enemy.
7. **Blocked names its unblocker**; **promotion is an event**, not
   an inference — emit `{"type": "promoted", "task": ...}` when a fan-in
   completes.
8. **Project containment is not dependency.** Give each task one truthful
   visual home; keep every blocker in `deps`, including cross-project edges.
   Never duplicate a task into two projects to make both impacts visible.
9. **Operator messages retain their authority and id.** The pane delivers a
   `[DAGR OPERATOR MESSAGE]` envelope to you. Respect `recommend_and_return`
   versus `may_decide_and_continue`; preserve `message_id` in the resolution
   event. dagr transports the request but does not act on it for you.

## Recipes

Every recipe below has a complete, strict-clean companion document under
`examples/` in this skill directory — held clean by the dagr test suite.
The fragments here show the *shape*; when you assemble a real file, crib
from the example, because the fragments alone omit cross-references (a
`cause.ref` needs its referent declared, a working attempt needs
locator + liveness) that `dagr check` will hold you to.

### Initialize a run

```json
{
  "dagr": 3,
  "run": {
    "id": "run-myjob-v01", "title": "what this run is",
    "started_at": "2026-02-01T09:00:00Z",
    "orchestrator": {"pane": "wX:p1"}
  },
  "generated_at": "2026-02-01T09:00:00Z",
  "projects": [],
  "tasks": [],
  "events": []
}
```

Refresh `generated_at` on every write — it anchors every "Nm ago" on
screen, and a stale value renders a staleness banner.

Set `run.orchestrator` automatically from your own `$HERDR_PANE_ID` when
available (or a stable Herdr agent target otherwise). This is where `m`
queues operator messages; do not point it at a worker. No user onboarding
step or extra controller is required.

### Shape projects before tasks

Use the smallest hierarchy that provides honest visual homes. Do not create
separate `phases` or `workstreams` arrays:

```json
"projects": [
  {"id": "APP", "title": "Application"},
  {"id": "API", "title": "API stream", "parent": "APP", "owner": "api-lead"},
  {"id": "UI", "title": "UI stream", "parent": "APP", "owner": "ui-lead"}
]
```

A task in `API` uses `"project": "API"`. If a UI task depends on it, keep
the UI task in `UI` and put the API task id in its `deps`; the renderer shows
the cross-project edge. Do not duplicate the task or force it into the
common parent. Omit `project` only for genuinely run-level work.

Start coarse: declare the useful project skeleton, immediate work, and
meaningful gates. Add discovered tasks as they become relevant; publish
operator-visible work, not every internal agent, tool, or runtime step.
Dagr derives queued-row `waits`, `ready`, `unassigned`, and `needs answer`
from `deps`, `owner`/`actor`, and `kind`; never encode those as extra fields.

### Open a task and start its first attempt

```json
{
  "id": "L1", "title": "impl: core lane", "kind": "impl",
  "owner": "l1-dev", "state": "working", "deps": [],
  "attempts": [{
    "id": "L1·a1", "n": 1, "cause": {"type": "initial"},
    "actor": "l1-dev", "model": "fable",
    "locator": {"pane": "wX:p3"},
    "state": "working", "started_at": "2026-02-01T09:05:00Z",
    "liveness": {"prompt_acknowledged": true, "last_output_at": "2026-02-01T09:05:00Z"}
  }]
}
```

Append `{"at": ..., "type": "attempt_started", "task": "L1", "attempt": "L1·a1", "actor": "l1-dev"}`
to `events`.

`model` is a free string rendered verbatim — dagr never rewrites it, so
YOU pick the display form. Use a short `model·effort` chip: `fable·xhigh`,
`sol5.6·max`, `luna5.6·max`, `terra5.6·max`. The pane gives the column 12
cells (`terra5.6·max` fills it exactly); longer strings get truncated with
an ellipsis, and narrow layouts drop the chip before they clip your title.

### Settle an attempt (with proof)

Set attempt `state`, `ended_at`, and `outcome` (`result` must equal the
state); mirror the task `state`; append an `attempt_settled` event.
(`ended_at` is when the *work* stopped; the settled event's `at` is when
the *verdict* landed — they may differ, and the gap is real information:
a pane that stopped at 09:50 whose rejection landed at 10:12 records
both.) Prefer the strongest evidence you have.

**The decision rule**: the tier describes your evidence for the
settlement claim. `done` requires that *someone claimed completion* —
however weakly; the tier grades the claim's evidence. When **nobody
claimed anything** and you are inferring from a runtime signal (pane
exit, silence, a green prompt), the state is `settled_unverified` — that
is what the state is for. The four honest settlements, spelled out
completely in
[`examples/06-evidence-tiers.json`](examples/06-evidence-tiers.json):

- **verified** — mechanical receipt:
  `{"result": "done", "evidence": "verified", "receipt": "cargo test 40/40 ✓ @ a1b2c3d"}`
- **reported** — a *typed envelope* from the actor (structured result
  data, not chat prose):
  `{"result": "done", "evidence": "reported", "receipt": "result envelope: {files: 4, status: complete}"}`
- **asserted** — the actor claimed completion, but only as prose:
  `{"result": "done", "evidence": "asserted", "reason": "actor asserted completion in chat; no typed envelope to verify against"}`
- **nobody claimed anything** — that is not a soft `done`, it is the
  distinct terminal state `settled_unverified`, and it still needs
  `ended_at` and a matching outcome:
  `"state": "settled_unverified", "ended_at": ..., "outcome": {"result": "settled_unverified", "evidence": "heuristic", "reason": "no claim of completion from the actor; inferring from clean pane exit"}`.
  It never upgrades unproven work to success.

### Send back and re-enter

A reviewer rejecting work touches THREE records — the review attempt
settles `done` (the review itself succeeded), the reviewed attempt
settles `rejected`, and the fix round is a **new attempt** whose `cause`
points at the review attempt:

```json
{"id": "L1·a2", "n": 2,
 "cause": {"type": "sent_back", "by": "rev-1", "ref": "R1·a1", "reason": "error paths untested"},
 "locator": {"pane": "wX:p3"}, "state": "working", "started_at": "...",
 "liveness": {"prompt_acknowledged": true, "last_output_at": "..."}}
```

`cause.ref` must name a **declared, earlier** attempt — if `R1·a1` isn't
in the file, the check fails; a working retry still needs locator +
liveness. Task back to `state: "working"`; dagr draws the ↩ re-entry
from exactly this cause. Complete document:
[`examples/03-send-back.json`](examples/03-send-back.json).

### Gate a fan-in

A gate's fan-in **is its `deps`** — that keeps every gate edge inside the
cycle check. Use `inputs` only when the fan-in set genuinely
differs from the dependency set:

```json
{"id": "G1", "title": "gate: merge lanes", "kind": "gate", "owner": "orchestrator",
 "project": "APP", "criteria": "API and UI reviews are clean",
 "state": "queued", "deps": ["L1", "L2", "L3"], "attempts": []}
```

Declare gate inputs in the intentional human reading order, and keep the
whole `tasks` array intentional too: dagr preserves declaration order for
attempt-less siblings and for the gate's state-bearing join strip. Do not
rename ids for sorting, attach the gate to one lane as a layout workaround,
or add a synthetic "join" task. Declare the truthful fan-in. A gate with
`project` is a milestone in that project; without one, dagr places it at the
nearest project shared by all inputs. Therefore a gate local to `API` stays
inside `API`, a gate joining `API` and `UI` lives in their parent `APP`, and a
gate joining unrelated top-level projects is a run-level milestone. Input
attempt timestamps never choose its parent. Each direct input renders as
`○` waiting, `◎` working, or `●` satisfied (plus the normal
blocked/review/failure marks). On narrow panes it aggregates those marks; a
selected gate still reveals the exact input ids.

Prefer an explicit `project` when the organizational ownership is known;
omit it when inference from input homes is the truthful answer. Never add a
fake dependency solely to move a gate on screen.

When the last input lands, append a `promoted` event — **with its `at`
timestamp**, like every event:

```json
{"at": "...", "type": "promoted", "task": "G1", "detail": "fan-in complete: L1 ✓, L2 ✓"}
```

"Moving the gate forward" means the projection rules still apply: a gate
at `working`/`done` needs its **own attempt** in that state — open
`G1·a1` (with locator + liveness if live) when the gate's work starts,
and settle it with evidence like any other attempt. A gate with
`attempts: []` can be `queued`, or `canceled` when the gate was withdrawn.
Complete document:
[`examples/04-gate-promotion.json`](examples/04-gate-promotion.json).

### Cancel planned work

Set the task to `"state": "canceled"` and give a short `note`; keep all prior
attempts and events unchanged. There is no canceled attempt outcome. Because
cancellation is not success, update or cancel any task that still depends on it.

### Declare loop policy (futures), don't imply it

```json
"policy": {"rounds_max": 3, "futures": [
  {"on": "pass", "ref": "G1"},
  {"on": "fail", "node": {"id": "L1·a2", "title": "fix round", "actor": "l1-dev"}, "loop_back": true},
  {"on": "fail", "streak": 2,
   "node": {"id": "L1x·a1", "title": "escalate: fresh approach", "attribution": "predicted"},
   "after": "L1·a2", "source": "two-strikes rule"}
]}
```

`ref` points at an existing task (rendered `»`); `node` declares a
not-yet-real one (`○`, `⟲` with `loop_back`, `≈` when `attribution` is
`predicted`); `after` chains onto a sibling future node of the same
policy. dagr renders futures **only** from this block, and only for
working or blocked nodes. Complete document:
[`examples/05a-policy-declared.json`](examples/05a-policy-declared.json).

### Materialize a future (when the predicted round actually starts)

`policy` is **current intent, not history** — it is the one block you
edit in place. History lives in `attempts` and `events`; those are
append-only, the policy is a forecast you keep truthful. When a declared
future comes real, do all of this in ONE candidate document (then
check → rename):

1. **Remove the consumed future node** from `policy.futures`. Leaving it
   would collide with the real attempt's id, which the check rejects.
2. **Repair `after` chains** that targeted the removed node: a sibling
   future that chained `"after": "L1·a2"` now hangs off the task
   directly (drop its `after`) or off another still-future sibling.
3. **Append the real attempt** with the id the future predicted, a
   `cause` pointing at the trigger (`followup` ref'ing the failed
   attempt for a loop-back fix round; `gate_failed` when a gate bounced
   it), locator + liveness if live.
4. Append the `attempt_started` event. Never touch prior attempts or
   events.

Before/after pair, both strict-clean:
[`examples/05a-policy-declared.json`](examples/05a-policy-declared.json) →
[`examples/05b-policy-materialized.json`](examples/05b-policy-materialized.json).

### Record a human decision

```json
{"at": "...", "type": "directive", "verb": "reject", "by": "operator",
 "task": "L1", "detail": "error paths untested"}
```

Verbs: `reject · unblock · answer · rule`. Directives are the decisions
log; chat prose is not.

### Answer a question (settle a task by directive)

A queued `question` whose dependencies are done appears as `needs answer` in
the attention queue; the producer declares no separate readiness field.
A directive event alone cannot settle a task — task state is a
projection over **attempts**, so the human's answer needs an
attempt whose actor is the human. Write both in the same candidate:

```json
{"id": "Q1", "title": "question: retry budget?", "kind": "question",
 "owner": "operator", "state": "done", "deps": [],
 "attempts": [{
   "id": "Q1·a1", "n": 1, "cause": {"type": "initial"},
   "actor": "operator", "state": "done",
   "started_at": "2026-02-01T09:40:00Z", "ended_at": "2026-02-01T09:42:00Z",
   "outcome": {"result": "done", "evidence": "reported",
               "receipt": "directive answer: retry budget 2 rounds"}}]}
```

plus the event:

```json
{"at": "2026-02-01T09:42:00Z", "type": "directive", "verb": "answer",
 "by": "operator", "task": "Q1", "detail": "retry budget 2 rounds"}
```

The receipt quotes the answer; the directive is the decision-log entry.
The same pattern settles any human-resolved task (an unblock that closes
a `question`, a rule that retires a task). Complete document:
[`examples/07-answer-question.json`](examples/07-answer-question.json).

### Handle an operator message

The pane's default action is one contextual message composer. Herdr queues
the finished message directly to the `run.orchestrator` locator, so do not
build another inbox daemon. You receive an envelope like:

```text
[DAGR OPERATOR MESSAGE]
message_id: msg-0123456789abcdef
run: run-myjob-v01
revision: 2026-02-01T09:42:00Z
target: G1
starter: get-guidance
authority: recommend_and_return

Ask sol5.6·max and fable·xhigh independently, then combine their opinions.
```

Do this:

1. Acknowledge the message. Treat the raw prose as instructions about the
   named target, bounded by the existing run scope.
2. Obey authority independently of prose:
   `recommend_and_return` means do the analysis and return the choice;
   `may_decide_and_continue` lets you decide and proceed. Never infer the
   second from wording such as “best guess”.
3. Treat model, reasoning, and multi-agent requests as ordinary editable
   instructions and use your normal orchestration tools. dagr runs none of it.
4. Keep the `message_id` through follow-ups. On resolution, append an event:

```json
{"at":"2026-02-01T09:50:00Z","type":"message_resolved","task":"G1",
 "message_id":"msg-0123456789abcdef",
 "detail":"recommended option B after two independent reviews; awaiting operator"}
```

If several messages informed one decision, set `source_messages` on the
directive/resolution event. Do not edit `messages.jsonl`; dagr owns that
append-only delivery journal. Your event is the durable project-memory link
back to it.

### Customize the three prompt starters when asked

There is no onboarding step. The built-ins (Use judgment, Get guidance,
Snooze) are available by default. If the user asks to add or change an action, atomically
write `actions.json` beside the run file:

```json
{
  "version": 1,
  "include_defaults": true,
  "actions": [
    {"id":"architecture-council", "label":"Architecture council",
     "prompt":"Ask two independent architecture reviewers and synthesize.",
     "authority":"recommend"}
  ]
}
```

Each action is only a prefilled editable prompt plus `recommend|decide`
authority. Keep the list small; prefer one flexible starter over many rigid
buttons. An id matching a built-in overrides it. `include_defaults: false`
replaces the built-ins. Config version `1` is the supported shape; at most
nine starters are shown, labels are capped at 80 bytes, and prompts at 32 KiB.
The pane reloads this file automatically and shows a banner for invalid or
unsupported configuration.

Old top-level run `actions` values are readable but inert. Do not add them.

### Lose a runtime

A pane died mid-work? The attempt is `lost` (no outcome needed), the task
projects to `failed` or `blocked`. Do not delete the attempt; the trace is
the record.

## What NOT to do

- Don't renumber, rewrite, or delete attempts or events. Append.
- Don't put herdr pane/agent ids in task or attempt ids — they're
  `locator` data, volatile by design.
- Don't claim `verified` without a mechanical receipt. Don't upgrade
  evidence after the fact without a new event explaining why.
- Don't encode task state from herdr's view of the world (pane alive ≠
  work done). herdr is *where*, the contract is *what is true*.
- Don't mirror every internal agent/tool step; include work an operator needs
  to understand, steer, or verify.
- Don't hand-compute "% complete" — write per-attempt `progress`
  (`{"done": 3, "total": 7, "note": "..."}`) and timestamps; analytics are
  queries over those.

## Field reference

The full schema is `CONTRACT.md` (Schema v3 section) in the dagr repo;
findings codes are listed there too. When `dagr check --json` names a code
you don't recognize, read its message — every finding carries the JSON
path of the offending field.
