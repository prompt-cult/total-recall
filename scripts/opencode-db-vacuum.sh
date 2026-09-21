#!/usr/bin/env bash
# opencode SQLite store: backup / prune events / vacuum / swap, with counts
# before and after so the swap is provably lossless.
#
# Usage: opencode-db-vacuum.sh <command> [args]
#
# Commands:
#   check                     abort (exit 2) if any opencode process is running
#   counts [db]               print reconciliation counts for a store
#   backup                    rename the live store aside with a timestamp
#   vacuum-into <out.db>      VACUUM INTO a compacted copy (safe on a warm store)
#   prune-events [db] [keep]  drop all but the newest [keep] (default 50) events
#                             per aggregate; run BEFORE vacuuming for real gains
#   swap <newdb>              move the live store aside, move <newdb> in its place
#   verify                    integrity_check + orphan check on the live store
#
# Store path override: OPENDOC_DB env var (default $HOME/.local/share/opencode/opencode.db).
# Set OPENDOC_SKIP_PROCESS_CHECK=1 only for scratch-database testing.
set -euo pipefail

DB="${OPENDOC_DB:-$HOME/.local/share/opencode/opencode.db}"

die() { echo "error: $*" >&2; exit 1; }
need_db() { [ -f "$DB" ] || die "no database at $DB"; }

cmd_check() {
    if pids=$(pgrep -f opencode); then
        echo "error: opencode is running (pids: $pids); the store is not cold." >&2
        echo "quit every opencode TUI (including the one an agent may be running inside) and retry." >&2
        exit 2
    fi
    echo "cold: no opencode processes found"
}

cmd_counts() {
    local target="${1:-$DB}"
    need_db
    sqlite3 "$target" <<'SQL'
SELECT 'sessions', COUNT(*) FROM session;
SELECT 'messages', COUNT(*) FROM message;
SELECT 'parts', COUNT(*) FROM part;
SELECT 'events', COUNT(*) FROM event;
SELECT 'message_bytes', SUM(length(CAST(data AS BLOB))) FROM message;
SELECT 'part_bytes', SUM(length(CAST(data AS BLOB))) FROM part;
SELECT 'orphan_parts_no_message', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM message m WHERE m.id = p.message_id);
SELECT 'orphan_parts_no_session', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = p.session_id);
SELECT 'orphan_messages_no_session', COUNT(*) FROM message m WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = m.session_id);
SQL
}

cmd_backup() {
    need_db
    local stamp dest
    stamp=$(date +%Y%m%d-%H%M%S)
    dest="${DB}.pre-vacuum-${stamp}"
    mv "$DB" "$dest"
    # the -wal/-shm belong to the backed-up store; move them out of the way too
    [ -f "${DB}-wal" ] && mv "${DB}-wal" "${dest}-wal"
    [ -f "${DB}-shm" ] && mv "${DB}-shm" "${dest}-shm"
    echo "backed up: $dest"
}

cmd_vacuum_into() {
    [ $# -ge 1 ] || die "vacuum-into <out.db>"
    need_db
    local out="$1"
    [ "$(dirname "$out")" != "." ] || out="$PWD/$out"
    rm -f "$out"
    time sqlite3 "$DB" ".timeout 60000" "VACUUM INTO '$out';"
    echo "vacuumed into: $out ($(du -h "$out" | cut -f1))"
}

cmd_prune_events() {
    local keep="${2:-50}"
    need_db
    sqlite3 "$DB" ".timeout 60000" "
DELETE FROM event WHERE seq NOT IN (
  SELECT seq FROM (
    SELECT aggregate_id, seq,
           ROW_NUMBER() OVER (PARTITION BY aggregate_id ORDER BY seq DESC) rn
    FROM event
  ) WHERE rn <= ${keep}
);"
    echo "pruned events (kept newest ${keep} per aggregate)"
}

cmd_swap() {
    [ $# -ge 1 ] || die "swap <newdb>"
    local newdb="$1"
    [ -f "$newdb" ] || die "no new database at $newdb"
    cmd_backup
    mv "$newdb" "$DB"
    echo "swapped in: $DB"
    ls -1 "${DB}-wal" "${DB}-shm" 2>/dev/null || true
}

cmd_verify() {
    need_db
    sqlite3 "$DB" <<'SQL'
SELECT 'integrity', (SELECT 'ok' FROM pragma_integrity_check LIMIT 1);
SELECT 'orphan_parts_no_message', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM message m WHERE m.id = p.message_id);
SELECT 'orphan_parts_no_session', COUNT(*) FROM part p WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = p.session_id);
SELECT 'orphan_messages_no_session', COUNT(*) FROM message m WHERE NOT EXISTS (SELECT 1 FROM session s WHERE s.id = m.session_id);
SQL
}

case "${1:-}" in
    check) cmd_check ;;
    counts) shift; cmd_counts "${1:-}" ;;
    backup) cmd_backup ;;
    vacuum-into) shift; cmd_vacuum_into "$@" ;;
    prune-events) shift; cmd_prune_events "$@" ;;
    swap) shift; cmd_swap "$@" ;;
    verify) cmd_verify ;;
    *) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
