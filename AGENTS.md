# AGENTS.md

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
