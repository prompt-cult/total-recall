# Contributing

## Read `AGENTS.md` first

[`AGENTS.md`](AGENTS.md) is the working contract for this repository: the
workflow, the commands, the scope rules, and the Andon prime directive. It
takes precedence over anything written here. Read it before you touch
anything.

## Workflow: Trunk Development Lite

Issue → branch → PR → squash-merge to `main` → pull. No review theatre for
routine work: **the test suite is the review.** Green tests on `main` are the
contract. Tag `main` only when it has been tested, and never leave a branch
hanging — merge it or kill it.

## The suite is the gate

```bash
cargo test                          # tests — write the failing test first
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --release
```

CI runs the same gates. A change is not done until all four are green. For a
docs-only change, run them anyway — they are cheap and they are the review.

## Commit style

History is one squash-merge per issue, so the commit message is the issue's
record. Conventional prefixes, as in `fix: unreadable rollout data is an error,
never an empty session`. `wip:` while a feature set is still landing.

Do not commit `*.env`, `.env`, or anything under `.tmp/`. Rollout fixtures
under `rollouts/` must be sanitised — no real prompts, keys, or paths — but
structurally faithful to the real formats.

## Never touch live session stores

The adapters read real session data under `$HOME` by default. For any
development or test run, arm the sandbox:

```bash
export TOTAL_RECALL_SANDBOX=1
export TOTAL_RECALL_<HARNESS>_ROOT=.tmp/<scratch>/<harness>
```

With the sandbox armed and no override set, adapter construction fails closed.
There is no reason to point a dev build at a live store.

## Licence

Dual-licensed `MIT OR Apache-2.0` — see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE). By contributing you agree your work is
available under those terms.

Behaviour expectations are in [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md);
security issues go through the private channel in
[`SECURITY.md`](SECURITY.md), not a public issue.
