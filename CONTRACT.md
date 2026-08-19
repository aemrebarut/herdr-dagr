# The dagr run-state contract — v2

> **Status: v2 (2026-08-18), with v1 read compatibility.** The v1 object
> model (§§1–8) remains valid; v2 adds recursive projects, scope-correct gate
> placement, an orchestrator locator, and correlated operator messages. It is the union
> of the requirements produced by a field survey of existing orchestration
> surfaces and by first-hand study of real multi-agent sessions, and each
> item cites why it exists; the field-level encoding is the Schema v2
> section below, enforced by `dagr check` and exercised by
> [`samples/run.json`](samples/run.json)
> — which encodes the full reference scene (executed loop,
> typed futures, fan-in gate, live review) as data. Existing `"dagr": 1`
> documents remain accepted and need no migration; the run is simply their
> only project scope. `dagr` renders
> exactly this contract and nothing else: if a fact isn't in the contract,
> the renderer must not invent it.

## Design stance

`dagr` is a **representation kernel, not an enforcement kernel**. It carries
no fencing, no CAS, no capabilities, and no scheduler/action engine. The producer (a file
written by hand, an orchestrator, eventually a ledger daemon) owns authority
and settlement. The contract's job is to make the true shape of the run
*expressible* — the two gaps found across every orchestration surface
studied (no task/attempt split, no dependency promotion) are
representation failures before they are enforcement failures.

Corollaries:

- The renderer is a pure function of `(contract state, width)`.
- Anything the producer did not assert is rendered as absent or `≈`/`!`
  tier — never silently upgraded.
- herdr identities (pane IDs, agent IDs) are **volatile locators recorded on
  attempts**, never node identity. Pane IDs do not survive workspace moves.

## Object model

### 0. Project — the recursive visual scope

The run is the implicit root project. Optional `projects[]` add named
recursive scopes with `id`, `title`, optional `parent`, `owner`, and `note`.
A task has at most one visual home through `task.project`; phases and
workstreams are not separate entities, just projects with a parent. A task
that affects two projects is not duplicated: it lives once, and its
dependency edges cross project boundaries visibly.

Project hierarchy answers “where is this shown?”; task dependencies answer
“what does this block?”. They are deliberately orthogonal. A cross-project
dependency is rendered as an off-tree `⇠` edge, not as false visual
containment. Project headings summarize attention in their descendants.

### 1. Task — the work item

Stable identity, independent of any attempt at it.

- `id` — producer-scoped stable ID (never a pane ID).
- `title`, `owner` (current), `kind` (impl / review / gate / question / …).
- `project` — optional visual home (§0); omitted means the run root.
- `state` — projection over its attempts: `queued · working · review ·
  blocked · done · failed · rejected · canceled · settled_unverified`.
- `deps` — see §3.
- Why: universal gap #1. Without this, "the agent finished" and "this
  attempt finished" are the same sentence. A retry must not move a task
  backward; it opens a new attempt.

**Projection rules (normative).** Task state is a claim about the attempt
record, and `dagr check` holds the two together (E150):

| task state | requires of attempts |
|---|---|
| `queued` | no working attempt; latest attempt (if any) not `done`/`settled_unverified` (backward moves are E150) |
| `working` | at least one `working` attempt |
| `review` | at least one attempt (reviewing nothing is W210) |
| `blocked` | no shape constraint — blocks are external facts; name `unblock` (W205) |
| `done` | latest attempt `done` |
| `failed` | latest attempt `failed` — or `lost` (a dead pane fails the task) |
| `rejected` | latest attempt `rejected` |
| `canceled` | no attempt constraint — planned work was withdrawn; existing attempts remain history |
| `settled_unverified` | latest attempt `settled_unverified` |

Terminal task states: `done · failed · rejected · canceled · settled_unverified`.
`lost` is an **attempt** state only (the runtime vanished); a task whose
latest attempt is lost projects to `failed` or `blocked`, and the queue and
trace surface the lost attempt itself. When both a task state and a live
attempt state exist, `blocked`/`review` on the task **outrank** the
attempt's `working`/`queued` for display — a blocked task with a working
attempt is blocked first.

### 2. Attempt — one try at a task

- `id` — `task·aN` style; attempts are **records, not a counter**.
- `cause` — why this attempt exists: `initial · sent_back(by, reason, ref) ·
  gate_failed(ref) · followup(ref, reason) · superseded(ref)`. The `↩` glyph
  and the focus card's cause chain render this verbatim. Causes point
  backward in time (E135/E136).
- `actor` — who/what ran it; plus `locator` — the volatile runtime address
  (herdr pane/agent ID) valid only while live, used for `[enter] focus`.
- `started_at`, `ended_at` — per-attempt timestamps. ETA, age, time-in-state,
  and rework-rate become queries, not regex over scrollback (the sessions
  studied all ended up hand-rolling duration analytics from timestamps
  retrofitted into prose).
- `outcome` + `evidence` — see §4.
- `progress` — optional sub-node progress (`3/7 files`, free-text step): in
  one studied orchestrator session a 35-minute stall was invisible because
  "working" was one opaque state with no intra-node progress at all.
- Optional `chain_key` — chained content identity
  (hash over inputs + predecessor keys) so replay/resume tooling can join
  against the trace and "longest unchanged prefix" is computable by a client.

### 3. Dependencies, gates, promotion

- `deps` — list of task IDs (`»` forward references in the rail; extra-dep
  `⇠` annotations off the primary tree).
- **Readiness is derived, not authored.** A queued task renders `waits ID`
  until every dependency is `done`; then it renders `ready` when assigned,
  `unassigned` without an owner/actor, or `needs answer` when its kind is
  `question`. A canceled dependency remains unmet; cancel or redirect its
  dependents rather than treating withdrawal as success.
- **Fan-in sets are first-class**: a gate's `inputs` carry per-input live
  state so the gate row can render `●◎●→⋈ G2` and name its blocker
  (`waits L5`).
- **Gates are milestones, not lane children.** A gate with `project` is drawn
  at that project. Without one, its scope is the nearest project shared by
  all inputs; inputs from unrelated top-level projects place it at the run
  root. An input's attempt history never changes this placement. The gate
  row is a boundary with a state-bearing `N→1 ⋈` join; selecting it reveals
  exact inputs. A retry may follow an earlier attempt of the same gate, but
  a gate never inherits ownership from whichever input ran last.
- **Promotion is an event, not an inference**: the producer emits a
  `promoted` event naming the `task` when a fan-in completes (v1 carries
  the completing gate in `detail`; a typed `by_completion_of` field is a
  v2 candidate). Universal gap #2 of the field. Edge lighting on selection
  derives from the declared `inputs`/`deps`, which promotion events attest.

### 4. Outcome evidence

Every terminal outcome carries a tier — never merged into the state:

- `◆ verified` — mechanically checked (test run, git-provenance receipt:
  commit descends from recorded base SHA, on-branch, in-scope — cadence's
  check, generalized).
- `◇ reported` — the actor asserted it through a typed envelope
  (orc's `HERDR_ORC_RESULT` instinct). A missing/unparseable envelope is
  `settled_unverified`, a *distinct* terminal state, not a soft `done`.
- `≈ heuristic` — inferred (screen-classified, in herdr-loop's terms).
- `! asserted` — bare claim, no structure.

Why: the convergent instinct of three independent plugin authors (loop's
detection tiers, cadence's receipts, orc's envelope) — "the agent said done"
is not evidence. Pane survival and herdr's `done` (an unseen-ready
projection) are **never** success evidence.

### 5. Loop policies — typed, emitted, not inferred

- A task may carry `policy`, in exactly the Schema v1 vocabulary:
  `futures[]` (each `on: pass|fail`, optional `streak` — fires after N
  consecutive fails — an existing-task `ref` XOR a declared `node`,
  optional `after` chaining onto a sibling future node of the *same*
  policy, `loop_back` for return-to-sender stubs, `source` naming the rule
  that authored it) · `rounds_max` (declared round budget) · `gate_cmd`
  (argv the gate runs; surfacing it is a v2 item).
- The renderer draws **dotted futures** (`├┄ ╰┄`, `⟲` loop-back stubs, `○`
  node stubs) *only* from declared policy. Stubs are earned only by
  working/blocked nodes — a queued or settled node shows no speculation.
  `≈` marks predicted attribution.

### 6. Liveness — first-class, not derived

Per live attempt:

- `prompt_acknowledged` — did the harness actually accept the last prompt
  (a studied session lost ~35 minutes to a prompt-delivery failure
  that looked identical to "working").
- `last_output_at` — staleness is renderable (`14m silent`).
- `queued_input` — composer lines typed but unsubmitted (a studied session
  was found with five lines sitting unsent).

herdr's event stream is a cache-invalidation signal for these hints, not a
work log; the contract state remains the producer's.

### 7. Human directives and decisions

Rejections, unblocks, answers, and rule changes are **entries with an author
and rationale** (`✗ operator "error paths untested"`), not chat prose. The
decisions log — the artifact operators of the studied sessions most wished
had been preserved — is a projection over these.

### 8. Event log (provenance)

Append-only `events`: attempt transitions, promotions, directives, with
timestamps and actors. Powers the focus-card provenance tail ("why does this
hold attention") and makes "why does a3 exist" answerable on screen.

### 9. Operator messages — contextual, durable, orchestrator-owned

`m` opens one editable composer targeted at the selected task/gate. dagr
ships three prompt starters—Use judgment, Get guidance, Snooze—but they are
only editable text plus a default authority. `Tab` changes starter and
`ctrl-t` changes authority independently:

- `recommend` / “return to me”: gather or think, then return a recommendation.
- `decide` / “may decide + continue”: the orchestrator may choose and proceed
  within the run's existing scope. It grants no broader authority.

The operator can freely add model names, thinking levels, independent-review
instructions, or any other prose. dagr never interprets it. The optional
file next to the run, `actions.json`, has this no-code shape:

```json
{
  "version": 1,
  "include_defaults": true,
  "actions": [
    {"id": "security-council", "label": "Security council",
     "prompt": "Ask two independent security reviewers and synthesize.",
     "authority": "recommend"}
  ]
}
```

Config version `1` is the supported shape. An id matching a built-in replaces
it; `include_defaults: false` starts from an empty set. The pane shows at most
nine starters and surfaces invalid/unsupported configuration in its banner
instead of silently changing behavior. Labels are capped at 80 bytes and each
editable prompt at 32 KiB.

The run declares `run.orchestrator {pane, agent}`. On Enter, dagr first
appends the immutable raw message to adjacent `messages.jsonl`, including
message id, run/revision, target, authority, text, and destination. Only then
does it call Herdr's native queued-input API (`agent.prompt`). Delivery or
failure is a second append-only record. A new journal is owner-only (`0600`
on Unix); it contains operator prose and should be treated as run data, not
committed casually. Reads are filtered by `run.id`, so sibling run files may
share the directory without showing each other's messages. There is no dagr polling engine:
monitor processes, background work, cron, agent selection, synthesis, and
execution remain the orchestrator's job.

The orchestrator correlates the eventual result by appending an event with
`message_id` (or `source_messages[]` when several messages informed it).
`message_resolved` requires a task, message id, and resolution detail;
ordinary `directive` events may also carry those correlations. The focus
card shows recorded/delivered/resolved state and the exact text, so a custom
instruction does not disappear after submission.

### 10. Legacy CLI actions — producer-declared, confirm-gated

For backward compatibility, an optional top-level `actions` block maps verbs to **argv templates**
naming the producer's own CLI. This is the only mutating surface the pane
has. It mutates nothing itself: `dagr` runs the producer's command and
renders whatever the producer then writes. Documents without the block are
unaffected (it is additive within v1; validators that predate it ignore
it — findings for it fire only when it is present).

```json
"actions": {
  "unblock": {"argv": ["mylegder-cli", "unblock", "{task}", "--by", "{operator}", "--key", "{key}"]},
  "answer":  {"argv": ["mylegder-cli", "answer", "{task}", "--text", "{text}", "--key", "{key}"]},
  "accept":  {"argv": ["mylegder-cli", "accept", "{task}", "--attempt", "{attempt}", "--key", "{key}"]},
  "reject":  {"argv": ["mylegder-cli", "reject", "{task}", "--attempt", "{attempt}", "--reason", "{text}", "--key", "{key}"]}
}
```

- Templates are **argv arrays, never shell strings** (same rule as
  `gate_cmd`). Placeholders: `{task}` `{attempt}` (selected attempt id)
  `{operator}` (`$DAGR_OPERATOR` or `$USER`) `{text}` (typed at the
  confirm prompt) `{key}` (idempotency key). Substituted values are
  single argv elements filled in one pass — a placeholder-shaped task id
  or typed text is data, never re-expanded.
- `{key}` is an identity for the **complete intent**: FNV-1a64 over a
  length-prefixed encoding of
  `(run.id, verb, task, attempt, generated_at, operator, text)` —
  length-prefixed so unrestricted strings cannot collide structurally
  across field joins, and computed only after `{text}` is typed, so the
  confirm gate shows the real key and a corrected reason is a *new*
  intent with a new key. Retrying the same confirmed command (crash,
  double-press) reuses the same key and the producer applies it once.
  `generated_at` is the document revision in the tuple; producers that
  may write twice within its resolution must dedupe on their own
  revision token instead of relying on the timestamp advancing.
- Every template **must carry `{key}`** (E192) — an action without a
  dedupe token turns a nervous double-confirm into a double mutation.
  `argv[0]` must be a **literal** executable name: nonempty, NUL-free,
  and placeholder-free (E193) — the executable is pinned by the
  template, never resolved from run data or the environment at confirm
  time. A document that declares `actions` must carry `generated_at`
  (E194): the key hashes the document revision, and without one every
  repetition of an intent keys identically forever, so a later
  same-shaped intent would be swallowed as a replay.
- **Trust boundary**: dagr executes `argv` directly — no shell is ever
  interposed, so typed text cannot break out of its argv slot. That
  guarantee ends where the template chooses to reintroduce an
  interpreter: `["sh", "-c", "… {text} …"]` is validator-clean but makes
  typed text shell code. Producers must not place placeholders inside
  interpreter command strings; the validator cannot police what the
  named executable does with its arguments.
- The pane binds `u`→`unblock`, `a`→`answer`, `o`→`accept`,
  `x`→`reject`, each **confirm-gated**: the exact argv (placeholders
  filled, key included) is shown before anything runs, and only `y`
  confirms. "Shown" is enforced, not assumed: the viewport pins the
  prompt on screen while a gate is open, `y` is inert until a draw
  proved the gate visible and it has been up long enough to be read,
  keys queued before the gate existed are drained, a paste is text for
  the text prompt only, and a document reload cancels the open gate
  rather than carrying the intent across a revision. Result = the
  producer's next write; `dagr` renders no local state change, ever.
- The verb set is open; unknown verbs are declared-but-unbound (W211).

## Transport

A single JSON document read from a path; watch = mtime poll or
producer-touched signal file. Herdr is used only for locator hints, focus,
and queued operator messages; it never supplies task truth. Canonical sample:
[`samples/run.json`](samples/run.json). (Earlier design studies used a
legacy flat shape that predates this contract; it is not part of v1.)

## Schema v2 — field reference

Top level: `dagr` (int, `1|2`; producers should write `2`) ·
`run {id*, title, started_at, orchestrator {pane, agent}}` ·
`generated_at` (staleness anchor) · `projects[]` · `tasks[]*` · `events[]` ·
`actions {verb → {argv[]*}}` (legacy optional surface, §10).
All timestamps ISO-8601. Unknown fields are ignored (forward compat).
`*` = required.

**Project** — `id*` (unique) · `title*` · `parent` (another project id;
omit for a run-root child) · `owner` · `note`. Parent edges must be acyclic.

**Task** — `id*` (unique, producer-scoped) · `title*` · `kind*`
(`impl|review|test|gate|question|docs|ship|…` — open set) · `owner` ·
`project` (visual home; omitted = run root) ·
`state*` (`queued|working|review|blocked|done|failed|rejected|
canceled|settled_unverified`) · `deps[]` (task ids) · `inputs[]` (gates: fan-in set,
defaults to `deps`) · `unblock` (blocked: who) · `note` · `criteria` · `policy` ·
`attempts[]`.

**Attempt** — `id*` (globally unique, `T·aN` style) · `n*` (1-based, unique
in task) · `cause {type: initial|sent_back|gate_failed|followup|superseded,
by, ref, reason}` (n>1 without cause is suspect) · `actor` · `model` ·
`locator {pane, agent}` (volatile, live only) · `state*` (`queued|working|
done|failed|rejected|settled_unverified|lost`) · `started_at`/`ended_at` ·
`outcome {result, evidence: verified|reported|heuristic|asserted, receipt,
reason}` (required when state is terminal; `result` must equal `state`;
missing `evidence` renders as `!`) · `progress {done, total, note}` ·
`liveness {prompt_acknowledged, last_output_at, queued_input}` ·
`chain_key` (optional chained content identity).

**Policy** — `rounds_max` · `gate_cmd[]` (argv, never a shell string) ·
`futures[]`. **Future** — `on* (pass|fail)` · `streak` (default 1) ·
exactly one of `ref` (existing task id → rendered `»`) or `node {id*,
title, actor, model, attribution: planned|predicted}` (rendered `○`, or
`⟲` with `loop_back: true`; `predicted` renders the `≈` prefix) · `after`
(chains onto another future node id) · `source` (rule provenance).

**Event** — `at*` · `type*` (`attempt_started|attempt_settled|promoted|
directive|message_resolved|note`) · `task`/`attempt` refs · `actor` · directives add
`verb (reject|unblock|answer|rule)` + `by` · `detail` · optional
`message_id` / `source_messages[]` correlations. `message_resolved` requires
`task`, `message_id`, and `detail`. Append-only,
ascending time.

**v1 migration:** none required. A v1 document has no named project scopes
or orchestrator message target. It receives corrected scope-level gate
placement immediately; add `"dagr": 2`, `run.orchestrator`, projects, and
task homes only when those features are wanted.

## dagr check — findings

Errors (exit 1): E001 not-a-document/parse · E100 version · E101 run.id ·
E102 tasks missing · E103 empty orchestrator locator · E104 duplicate/malformed
project · E105 unknown project parent · E106 project cycle · E107 task names
unknown project · E108 gate project excludes an input scope · E110/E130 duplicate task/attempt id · E111/E131
missing required fields · E112/E132 unknown state · E120/E121 dangling
dep/input · E122 dependency cycle over `deps` ∪ gate `inputs` (a run is
a DAG, and a fan-in override cannot smuggle a cycle past it) · E113 attempt id
colliding with a task id · E133/E134 bad cause type/ref · E135 cause
cycle · E136 cause referencing a later attempt (causes point backward in
time) · E140 terminal attempt without outcome · E141 result≠state ·
E142 unknown evidence tier · E150 task state contradicts attempt record
(the §1 projection table, exactly) · E160–E164 malformed futures (E162 covers
`after` outside the task's own policy and after-chain cycles; E164 covers
missing/colliding/duplicate future-node ids) · E170/E171 malformed/dangling
event · E172 event missing per-type required fields (subjects, directive verb/by) · E180 invalid timestamp (full ISO-8601 validation — calendar
ranges, offsets normalized to UTC; the renderer uses the same parser) ·
E181 attempt ends before it starts · E190 action template malformed
(missing/empty argv, non-string element — reported per element at its
path) · E191 action template uses an unknown placeholder · E192 action
template carries no `{key}` placeholder (no dedupe token) · E193
`argv[0]` not a literal executable name (empty, NUL-bearing, or
placeholder-bearing) · E194 `actions` declared without `generated_at`
(intent keys need a document revision) — findings for `actions` fire
only when the block is present.

Warnings (exit 0, or 1 with `--strict`): W100 no `generated_at` · W201
outcome without evidence tier · W202 gate whose resolved fan-in set
(`inputs`, defaulting to `deps`) is empty · W203 settled attempt without
timestamps · W204 working attempt without locator · W205 blocked without
unblock owner · W206 n>1 without cause · W207 events out of order · W208
working attempt with no populated liveness field · W209 settled task
with an attempt still working (unfenced stale attempt) · W210 `review`
task with no attempts (reviewing nothing) · W211 action verb declared but
not bound to any pane key (declared-but-unreachable).

## Non-goals

- No authority: `dagr` never settles, fences, or retries anything.
- No inference of structure: no scraping scrollback, no deriving deps from
  text, no guessing attempt boundaries.
- No herdr-derived work state: herdr tells us *where* things are running and
  *whether pixels moved*, never *what is true*.
