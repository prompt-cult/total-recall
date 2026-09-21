---
name: opencode-db-vacuum
description: Reclaim space in and safely swap the opencode SQLite session store. Use this skill when the opencode database has grown huge, when you need a provably-lossless backup/prune/vacuum/swap procedure with before-and-after counts, or to diagnose where SQLite file size is actually going (event log vs session data).
---

# opencode DB Vacuum

The opencode session store at `~/.local/share/opencode/opencode.db` grows
unboundedly: every message/part update appends a **full JSON snapshot** to
the `event` bus-log table. On a measured estate the event log was 31.5 GB of
a 36 GiB file while real session data (messages + parts) was only ~4.4 GB.
Plain `VACUUM` reclaims nothing there (`freelist_count` = 0) — the win is
pruning `event` first, then vacuuming.

Full measurements and the written procedure: `vacuum.md` in the total-recall
repository.

The helper script: `scripts/opencode-db-vacuum.sh` in the total-recall repo.
All commands take the store path from `OPENDOC_DB` (default
`~/.local/share/opencode/opencode.db`).

## The one rule

The store is only cold when **every opencode process is quit** — including
any agent TUI, because the agent runs inside opencode and cannot perform the
swap itself. The script enforces this (`check` exits 2 if opencode is
running).

## Workflow

From a plain terminal (not inside opencode):

```sh
SCRIPT=<path-to-total-recall-repo>/scripts/opencode-db-vacuum.sh

$SCRIPT check                # must pass: opencode quit, WAL checkpointed
$SCRIPT counts               # record the before numbers (orphan_* must be 0)
$SCRIPT verify               # integrity_check + orphans
$SCRIPT prune-events . 50    # optional; the actual space win: keep newest 50 events/aggregate
$SCRIPT vacuum-into "$HOME/.local/share/opencode/opencode.db.new"   # ~4 min for 36 GiB
$SCRIPT swap    "$HOME/.local/share/opencode/opencode.db.new"      # renames old aside, moves new in
$SCRIPT counts               # record the after numbers
$SCRIPT verify               # integrity ok, orphans 0
```

Swap moves `opencode.db` and its `-wal`/`-shm` aside as
`opencode.db.pre-vacuum-<stamp>`; the timestamped copy is the rollback.

## Reconciliation criteria

A swap is good when, comparing before/after `counts`:

- `integrity` = ok, all three `orphan_*` counts = 0
- `sessions` identical, `messages`/`parts` identical (a warm snapshot copy
  lags by whatever live sessions wrote during the vacuum — rehearsal
  measured −24 messages / −87 parts over 3m55s; drift is the snapshot
  window, not loss)
- `events` reduced only by the prune predicate
- `message_bytes`/`part_bytes` unchanged

## Rollback

```sh
cd ~/.local/share/opencode
mv opencode.db opencode.db.vacuumed
mv opencode.db.pre-vacuum-<stamp> opencode.db
mv opencode.db.pre-vacuum-<stamp>-wal opencode.db-wal 2>/dev/null || true
mv opencode.db.pre-vacuum-<stamp>-shm opencode.db-shm 2>/dev/null || true
```

## Measured rehearsal numbers

- Warm `VACUUM INTO`, 36 GiB source: 3m55s (I/O-bound)
- `integrity_check` on a 36 GiB copy: 2m00s
- Vacuum alone reclaimed 0.03% — freelist was 0; prune events first
- Event breakdown: `message.updated.1` 416k rows / 20.2 GB (avg 50.7 KB —
  full message snapshot per edit), `message.part.updated.1` 1.3M rows /
  11.1 GB
- `length()` on TEXT counts characters; use `length(CAST(data AS BLOB))`
  for byte truth
