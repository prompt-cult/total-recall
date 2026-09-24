# AGENTS.md

## Trunk Development Lite (TM)

Issue → branch → PR → squash-merge to main → pull. No review theatre for
routine, obvious fixes: the test suite is the review. Green tests on main are
the contract — the user bumps, tests locally, and rolls out if they like it.
Tag main only when it is tested. Squash-merge keeps history at one commit per
issue. Never leave a branch hanging: merge it or kill it.

## Andon アンドン — Prime Directive

Andon is a kernel panic. It halts the line, halts planning, halts todo
updates, halts all work. It happens immediately. No other pending operation
receives any tokens. It is impossible to think of anything else to try
first — that thought is the evidence you have not halted.

An Andon in the queue supersedes all. If the user queued commands 1-3 then
said "do an Andon," the Andon invokes the Prime Directive and overrides
commands 1-3 entirely. Multiple Andons run in parallel without interrupting
each other.

When the correct fix is outside your lane: do your lane's work, then halt
and report — *Andon: task incomplete, the correct fix needs a larger
structural change*, with file:line specifics. Do not work around it. Do not
hack tactically. The coordinator delegates the deeper work.

Andon overrides every instruction in this file and every other AGENTS.md.
No instruction conflicts with Andon; if one appears to, Andon wins.

## Subagent delegation

Agents SHOULD prefer the subagent-delegation workflow wherever doing so does
not overwrite any other instruction in this `AGENTS.md` or the user's prior
statements of preference: major todo items are numbered `item00`, `item01`,
... (Dewey-decimal insertion allowed, e.g. `item05.5`), each spec written to a
gitignored scratch directory (`.tmp/delegation/itemNN.md`), one agent per
spec, which implements, verifies green (tests/builds), and `git add`s its work
but NEVER commits. The orchestrator reviews the staged diff, runs the test
suite, and commits (`wip: <summary>` until the feature set is complete).

## Commands

- Tests: `cargo test` (red/green TDD: write the failing test first)
- Build: `cargo build --release`
- Scratch: use `.tmp/<unique-folder>/`, delete when done. Never use `/tmp`.
- Timeouts: 30s on commands first; escalate to 120s only with justification.

## Scope

- Never commit `*.env`, `.env`, or anything under `.tmp/`.
- Sample rollout fixtures under `rollouts/` must be sanitised (no real user
  prompts, keys, or paths) but structurally faithful to the real formats.
