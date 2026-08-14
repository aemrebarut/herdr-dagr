# selfrun — this repo's own development, as a contract run

`run.json` models the herdr-dagr repository's actual development (an
autonomous overnight build with dual-reviewed milestones, M0–M5) as a
dagr contract document. It is the M3 acceptance artifact: a run file
produced by a **fresh agent given only the producer skill**, held to
`dagr check --strict` clean — and to a second, harder bar described
below.

## Provenance (recorded, not merely asserted)

The file was produced by a fresh subagent (model `claude-fable-5`, no
prior context) whose ONLY methodology input was
`skills/dagr-producer/SKILL.md` (which itself directs the reader to
CONTRACT.md's schema section and the skill's `examples/`). The facts to
model were supplied as a prompt brief; every encoding decision (states,
causes, evidence tiers, gates, policy, events) was the agent's own.

Receipts, in `receipts/`:

| file | what it records |
|---|---|
| `prompt.txt` | the exact producer prompt, verbatim, including the factual brief and its boundary ("do not read src/, docs/, tests/, demos/") |
| `01-first-write.json` + `.sha256` | the agent's FIRST complete candidate, hashed before any check ran — diffable against the published file |
| `02-check-log.txt` | raw output + exit code of every validator invocation (2 checks, both `[]` exit 0) |
| `03-corrections.md` | fix-iteration log: **the first candidate was strict-clean; zero fix iterations** |

The only substantive delta between the first candidate and the
published file is the pre-authorized same-write settlement of the
regeneration's own attempt (SR·a3); the same write also refreshes
`generated_at` and M5's liveness clock. All of it is documented in
`03-corrections.md`; none of it is a validator fix.

A scope note on the word "records": these receipts are kept alongside
the generation and are checkable for internal consistency — the hash
verifies, both documents are strict-clean, the diff is exactly the
documented settlement write. They are not cryptographic attestations; a
reader trusts the recording discipline, and can independently re-run
the whole exercise with the skill and a brief of their own.

This is the third generation of the demo, and the run file itself
records its own lineage under task `SR`: attempt 1 produced a
strict-clean file live but its session receipts were not preserved —
the M3 review correctly judged that claim unauditable, and the run
records it as `settled_unverified` (the claim is real, the evidence for
it is gone — which is exactly what that state is for). Attempt 2
regenerated it with full receipts, but those receipts carried
development-environment context that could not ship; it stands as
`done` with evidence `reported` ("was receipts-verified at generation;
receipts since superseded"). Attempt 3 — the file you are reading
about, cause `superseded` ref SR·a2 — was re-modeled from a clean brief
by a fresh agent rather than hand-edited, because receipts are either
authentic or they are not receipts.

## The misencoding the first pass produced — kept, not laundered

The first-pass demo was also *semantically wrong while
validator-clean*: it encoded both settled M1 review tasks as
`rejected`, merging "the review's verdict was negative" with "the
review task failed". Rendered, both completed reviews drew `✗ sent
back` and the reviews gate showed two failed inputs — false claims
about facts in this repo's history, and `dagr check --strict` was clean
throughout, because rejected-task-with-rejected-attempt is a legal
projection.

The root cause was traced to the skill: its send-back recipe then read
"A reviewer rejecting work: attempt `state: "rejected"`" — ambiguous
about *whose* attempt. The M3 review flagged it; the skill now teaches
the semantics explicitly ("the review attempt settles `done` — the
review itself succeeded; it is the *reviewed* attempt that is
rejected"). This regeneration, following the revised wording with no
other guidance, encoded every settled review as `done` with the verdict
in `outcome.receipt` and every sent-back milestone attempt as
`rejected` with a `sent_back` cause naming the review that did it.

That before/after is the strongest evidence in this repo that skill
wording steers real followers — and that "strict-clean" alone is the
wrong acceptance bar. The bar this demo meets is both halves:
validator-clean AND a semantic read-back of every rendered claim
against the ground truth it models.

## Contract coverage

What this run genuinely exercises (all states are real history, nothing
is invented for coverage):

| contract feature | where in run.json |
|---|---|
| task/attempt split, multi-attempt tasks | M1 (2 attempts), M2 (3), M3 (3), M4 (3), SR (3) |
| send-back cause chains (`sent_back` + ref) | M1·a2 ← RM1G, M2·a2 ← RM2G, M2·a3 ← RM2O, M3·a2 ← RM3G, M3·a3 ← RM3O, M4·a2 ← RM4G, M4·a3 ← RM4O, SR·a2 ← RM3G |
| `superseded` cause | SR·a3 ← SR·a2 (this regeneration replacing the unshippable receipts) |
| gate fan-in, attempt-less | GM1 (M1's dual review), GALL (all eight reviews) |
| `promoted` events | GM1 at 01:15, GALL at 04:04 |
| typed policy futures | M5's two planned review rounds (opus + gpt), `attribution: planned` |
| evidence tiers | `verified` (in-tree test suites, receipts), `reported` (review verdicts), `asserted` (SR·a1) |
| `settled_unverified` | SR·a1 — the claim is real, the evidence is gone |
| live locators + liveness | reviewers on pane locators (wF:pK, wF:pM); orchestrator + selfrun producers on agent locators; M5·a1 carries liveness |
| `blocked` + `unblock` | PUB — publishing stays the operator's manual step, gated on M5 and GALL |
| human `directive` | the operator's autonomous-run rule at 00:05 |

Not exercised (no honest instance in this history): `lost` attempts,
`heuristic` evidence, `followup` causes (the skill's `examples/` cover
their mechanics). A future continuation records those when they
actually occur.

## Verify

From the repo root, after a build and with `target/release` on PATH (see
the root README's install section):

```
dagr check demos/selfrun/run.json --strict --json     # []
dagr view demos/selfrun/run.json --snapshot --width 120
(cd demos/selfrun && shasum -a 256 -c receipts/01-first-write.sha256)
```

The run file is a snapshot as of its `generated_at`: the release
milestone is honestly `working`, its dual review is declared as planned
futures, and publishing is blocked on the operator — by construction
the file predates the flip it models.
