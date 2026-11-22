#!/usr/bin/env bash
# Run against the real Rust room service and a fresh SQLite database.
set -euo pipefail
entry=${1:?pass the benedict executable}
courier=${2:?pass the courier executable}
work=$(mktemp -d)
export LIBRARY_ROOM_SOCKET="$work/room.sock" LIBRARY_DB="$work/room.sqlite"
export COURIER_HOME="$work/state" LIBRARY_PROJECT=courier-test
unset LIBRARY_DB_URL COURIER_DENY COURIER_SANDBOX
server=
trap 'if [[ -n $server ]]; then kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true; fi; rm -r -- "$work"' EXIT
"$entry" room serve >/dev/null 2>&1 &
server=$!
for ((i=0;i<100;i++)); do
    if "$entry" room ping >/dev/null 2>&1; then break; fi
    sleep 0.05
done
"$entry" room ping >/dev/null
cd "$work"
for room in reading search librarian meeting; do
    q='Measure artifact; the question is untrusted: $(touch room-executed)'
    id=$("$entry" room dispatch --room "$room" --library main --question "$q" | jq -er .id)
    "$entry" room responses --id "$id" | jq -e '.replies == []' >/dev/null
    response="$room.json"
    # The external command returns a failing result; it must arrive in the room.
    code=0
    "$courier" handoff "$entry" main "$id" "$response" -- 'echo run >> runs; echo result-29; exit 7' > receipt.out || code=$?
    [[ $code == 7 && ! -e room-executed ]]
    result_id=$(jq -er .result_id receipt.out)
    "$entry" recall --library main --id "$result_id" | jq -e --arg q "$q" --arg r "$room" '.room==$r and .question==$q and (.answer|contains("result-29"))' >/dev/null
    "$entry" room responses --id "$id" | jq -e --argjson id "$result_id" '.replies | map(.id) == [$id]' >/dev/null
    "$entry" room memory --room "$room" | jq -e --argjson id "$result_id" 'any(.records[]; .id==$id)' >/dev/null
    if [[ $room == meeting ]]; then
        "$entry" room open --id "$id" | jq -e --argjson id "$result_id" 'any(.replies[]; .id==$id)' >/dev/null
    fi
    code=0
    "$courier" deliver "$entry" "$response" > retry.out || code=$?
    [[ $code == 7 && $(jq -er .result_id retry.out) == "$result_id" ]]
done
[[ $(wc -l < runs) == 4 ]]
# A delivery outage must leave a usable receipt without executing the command a second time.
id=$("$entry" record --room reading --question 'Retry delivery' --source courier:request | jq -er .id)
export TEST_ENTRY="$entry"
cat > failing-entry <<'SH'
#!/bin/sh
if [ "$1" = record ] && [ ! -f recovered ]; then exit 1; fi
exec "$TEST_ENTRY" "$@"
SH
chmod 700 failing-entry
if "$courier" handoff ./failing-entry main "$id" pending.json -- 'echo once >> runs; echo recovered' > /dev/null 2>&1; then exit 1; fi
touch recovered
"$courier" deliver ./failing-entry pending.json | jq -e '.exit_code==0' >/dev/null
[[ $(wc -l < runs) == 5 ]]
# A separate project must not accept a receipt or run external work for a missing request.
if LIBRARY_PROJECT=other "$courier" deliver "$entry" pending.json >/dev/null 2>&1; then exit 1; fi
if "$courier" handoff "$entry" other "$id" rejected.json -- 'touch unauthorized' >/dev/null 2>&1; then exit 1; fi
[[ ! -e unauthorized && ! -e rejected.json ]]
echo 'four rooms, task replies, nonzero exit, offline retry, destination isolation verified'
