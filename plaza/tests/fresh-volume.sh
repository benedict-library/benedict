#!/bin/sh
# Run against a built image. Docker gives this test a fresh anonymous /data volume.
set -eu
image=${1:?usage: fresh-volume.sh IMAGE}
docker run --rm -i --network none --entrypoint sh "$image" -s <<'CHECK_VOLUME'
set -eu
test "$(id -u)" = 1001
test "$(id -g)" = 1001
test -w /data
test ! -e /data/events.jsonl
PLAZA_TOKEN=$(head -c 32 /dev/urandom | base64)
export PLAZA_TOKEN PLAZA_ACCOUNTS=/tmp/smoke-accounts.json PLAZA_URL=http://127.0.0.1:43190
umask 077
printf '[{"account":"reader","persona":"Reader","token":"%s"}]\n' "$PLAZA_TOKEN" > "$PLAZA_ACCOUNTS"
start_server() {
  plaza serve >/tmp/plaza-server.log 2>&1 &
  server_pid=$!
  tries=0
  until wget -q -O /dev/null http://127.0.0.1:43190/healthz 2>/dev/null; do
    tries=$((tries + 1))
    test "$tries" -lt 40 || { cat /tmp/plaza-server.log >&2; exit 1; }
    sleep 0.1
  done
}
start_server
plaza ask 'Which edition?' --tag sources >/dev/null
test -s /data/events.jsonl
kill "$server_pid"
wait "$server_pid" 2>/dev/null || :
start_server
plaza show p1 --json | grep -q 'Which edition?'
kill "$server_pid"
wait "$server_pid" 2>/dev/null || :
printf 'PASS uid=1001 volume_write=1 post_after_restart=1\n'
CHECK_VOLUME
