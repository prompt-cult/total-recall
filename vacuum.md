# opencode store: vacuum rehearsal and procedure

The opencode session store (`~/.local/share/opencode/opencode.db`) was 31 GiB
and looked bloated: message+part payloads sum to only ~4.4 GB. This document
records a dress rehearsal of a vacuum/swap, what it disproved, what the real
space consumer is, and the verified procedure for shrinking the store safely.

## Measured facts (2026-09-20/21 rehearsal)

| Measurement | Value |
|---|---|
| Live store size at first look | 31 GiB (grew to 36.3 GiB over the session — it grows while opencode runs) |
| Warm `VACUUM INTO` timing (36 GiB source) | 3m55s wall, 8.7s user, 114s system, 52% CPU — I/O-bound |
| Rehearsal copy size after `VACUUM INTO` | 36.30 GiB (reclaimed ~0.03%) |
| `PRAGMA freelist_count` on live store | **0** — no free-page bloat whatsoever |
| `PRAGMA page_size` | 4096 |
| `PRAGMA integrity_check` on the copy | `ok` (2m00s) |
| Snapshot drift, live vs copy after 3m55s | live had +24 messages, +87 parts — copy is a consistent snapshot, not a mirror |

### Where the space actually goes (dbstat, copy)

| Table / object | Size |
|---|---|
| `event` | **31.5 GB** (86%) |
| `part` | 2.5 GB (733k rows, payload 2.48 GB true bytes) |
| `message` | 1.9 GB (186k rows, payload 1.99 GB true bytes) |
| `event` indexes | ~0.3 GB |
| everything else | < 0.1 GB |

Earlier `length(data)` sums understated payloads because `length()` on TEXT
counts characters, not bytes — always use `length(CAST(data AS BLOB))` for
byte truth.

### The event log is the whale

1,835,459 event rows. opencode writes a **full JSON snapshot per update**:

| type | rows | data | avg row |
|---|---|---|---|
| `message.updated.1` | 416,655 | 20.2 GB | 50.7 KB |
| `message.part.updated.1` | 1,305,295 | 11.1 GB | 8.9 KB |
| `session.updated.1` | 110,862 | 76 MB | 0.7 KB |

Every message/part edit appends a complete JSON snapshot — the log grows
unboundedly as sessions run. The `event` table has no time column
(`id, aggregate_id, seq, type, data`), so age-based pruning needs care.

**Consequence: plain `VACUUM` reclaims nothing on this store** (freelist is
0). Space is only reclaimed by pruning `event` first, then vacuuming. This
is an opencode upstream growth issue, not user data: sessions/messages/parts
(what total-recall reads) are 4.4 GB of the 36.

## Procedure

Run from a plain terminal. The store is cold only when every opencode TUI is
quit — including the one an agent would be running inside, so an agent cannot
execute the swap itself; it can only verify before/after.

### 0. Guard — cold check

```sh
pgrep -fl opencode   # must output nothing
```

### 1. Counts before (write down)

```sh
sqlite3 ~/.local/share/opencode/opencode.db <<'SQL'
SELECT 'sessions', COUNT(*) FROM session;
SELECT 'messages', COUNT(*) FROM message;
SELECT 'parts', COUNT(*) FROM part;
SELECT 'events', COUNT(*) FROM event;
SELECT 'message_bytes', SUM(length(CAST(data AS BLOB))) FROM message;
SELECT 'part_bytes', SUM(length(CAST(data AS BLOB))) FROM part;
SELECT 'orphan_parts(no message)', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM message m WHERE m.id = p.message_id);
SELECT 'orphan_parts(no session)', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = p.session_id);
SELECT 'orphan_messages(no session)', COUNT(*) FROM message m WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = m.session_id);
SQL
```

Expect: 0 orphans in all three. Record all numbers.

### 2. Backup (rename the old store aside)

Quitting opencode closes the WAL; a checkpoint happens on clean exit. Verify
`opencode.db-wal` is gone (or tiny) before proceeding.

```sh
cd ~/.local/share/opencode
mv opencode.db opencode.db.pre-vacuum-$(date +%Y%m%d-%H%M%S)
```

### 3. Vacuum to a copy, swap the compact one in

```sh
sqlite3 opencode.db.pre-vacuum-* "VACUUM INTO '$HOME/.local/share/opencode/opencode.db.new';"
mv opencode.db.new opencode.db
```

Measured: ~4 min for 36 GiB on this machine; expect the output to be ~36 GiB
unless events were pruned first (step 4).

### 4. (Optional, the actual win) Prune the event log before vacuuming

The event log is a bus log — the TUI reads current state from the
session/message/part tables, not from historical events. Prune keeps the
newest N events per aggregate and drops the rest (default below keeps 50):

```sh
sqlite3 opencode.db.pre-vacuum-* "
DELETE FROM event WHERE seq NOT IN (
  SELECT seq FROM (
    SELECT aggregate_id, seq,
           ROW_NUMBER() OVER (PARTITION BY aggregate_id ORDER BY seq DESC) rn
    FROM event
  ) WHERE rn <= 50
);"
```

Run `VACUUM` (or step 3's `VACUUM INTO`) **after** this; then the output
should be ~5 GiB instead of ~36.

### 5. Verify after — counts must reconcile

Re-run the step-1 counts against the new store. Acceptance criteria:

- `integrity_check` → `ok`
- orphans still 0
- sessions: identical (no sessions lost)
- messages/parts: identical on a cold run (a warm snapshot copy will be
  behind by whatever the live sessions wrote during the vacuum — rehearsal
  measured −24 messages / −87 parts; that drift is the snapshot window, not
  data loss)
- events: only reduced if you pruned, and only by the prune predicate

Rehearsal verification on the copy: integrity ok, 0 orphans everywhere,
sessions identical (3205), messages/parts behind by the snapshot drift.

### 6. Rollback

```sh
cd ~/.local/share/opencode
mv opencode.db opencode.db.vacuumed
mv opencode.db.pre-vacuum-<stamp> opencode.db
```

opencode recreates `opencode.db-wal` and `opencode.db-shm` on next launch;
never copy stale `-wal`/`-shm` files onto a swapped-in database.

## Rehearsal artifacts

Rehearsal copy: `.tmp/opencode-vacuumed.db` (36.3 GiB, integrity ok, snapshot
of 01:07 local). Delete it after the real run; it is not needed for anything
the cold procedure will rebuild.
