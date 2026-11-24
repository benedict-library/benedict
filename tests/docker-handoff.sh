#!/usr/bin/env bash
# Real entry points, a new volume, and one ephemeral container per call.
set -euo pipefail
cd "$(dirname "$0")/.."
project="benedict-check-$(date +%s)-$$"
export LIBRARY_IMAGE="$project:local"
compose=(docker compose -p "$project")
library() { "${compose[@]}" run --rm -T library "$@" </dev/null; }
cleanup() { "${compose[@]}" down --volumes >/dev/null; }
trap cleanup EXIT
"${compose[@]}" build library
"${compose[@]}" run --rm -T -e LIBRARY_START=example library setup --activate |
  jq -e '.database_ready == true' >/dev/null
"${compose[@]}" run --rm -T --entrypoint sh library -ec '
  test -f /app/rooms/room_read.py && test ! -e /app/rooms/room_query.py
  test -r /shelf/river-field-note.md && test ! -w /shelf/river-field-note.md
  cmp /app/examples/golden.jsonl /data/golden/main.jsonl'
source=shelf:river-field-note.md
library read "$source" | jq -e '.text | contains("violet")' >/dev/null
question='Why use violet markers?'
entry=$(library record --room reading --question "$question" --answer 'The camera distinguished violet markers from existing orange markers.' --source "$source" | jq -er '.id')
page=$(library file --id "$entry" | jq -er '.path')
library search 'violet markers' | jq -e --arg p "$page" 'any(.hits[]; .path == $p)' >/dev/null
library room memory --room reading | jq -e --argjson id "$entry" 'any(.records[]; .id == $id)' >/dev/null
concept_id=$(library record --room librarian --question 'Marker visibility' --answer 'The camera distinguishes violet from orange.' --source "$page" | jq -er '.id')
concept=$(library file --as concept --id "$concept_id" | jq -er '.path')
"${compose[@]}" restart room
library recall --id "$entry" | jq -e --arg q "$question" --arg s "$source" '.question == $q and .source == $s' >/dev/null
library read "$page" | jq -e --arg s "$source" '.text | contains($s)' >/dev/null
library search 'marker visibility' | jq -e --arg p "$concept" 'any(.hits[]; .path == $p)' >/dev/null
library read "$concept" | jq -e --arg p "$page" '.text | contains($p)' >/dev/null
printf '%s\n' 'PASS shelf read, room memory, filing, search, durable recall after room restart'
