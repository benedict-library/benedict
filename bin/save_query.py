#!/usr/bin/env python3
"""Save one answered question as a page in a library's filing cabinet (wiki queries/) and one line in log.md.

usage: save_query.py <library key> "question" "answer" ["ref1.md,ref2.md"]

The library's wiki_root and save_block rules (name -> regex; any hit refuses the save) come from the
library registry table library.registry, read with psql through DATABASE_URL. Without the registry row
the save fails — there is no fallback path.
"""
from contextlib import contextmanager
import datetime
import json
import os
import re
import subprocess
import sys
import stat
import uuid
from pathlib import Path

STOP = {'무엇', '뭐', '어디', '어떻게', '왜', '언제', '누가', '이거', '그거', '저거', '하나', '인가', '인지', '되나', '있나',
        '대해', '관련', '방법', '어떤', '정도', '지금'}
REGISTRY_SQL = ("SELECT row_to_json(r) FROM (SELECT key, wiki_root, save_block, options FROM library.registry "
                "WHERE key = :'key' AND status = 'active') r")


def slug(q):
    """Question -> file name slug: letters, digits and _ only, spaces become _, 40 chars."""
    return re.sub(r'[^가-힣a-zA-Z0-9_ ]', '', q or "").replace(' ', '_')[:40] or "query"


def tags(q):
    toks = [t for t in re.split(r'[^0-9a-z가-힣]+', q.lower()) if len(t) >= 2 and t not in STOP]
    return ', '.join(dict.fromkeys(toks))[:120]


def summary(a):
    a = a.strip().replace('\n', ' ')
    m = re.search(r'^(.+?[.。])\s', a + ' ')
    return (m.group(1) if m else a)[:200]


def atomic_text(path, text, *, mode=None, creation_mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    name = path.parent / ("library-write-" + uuid.uuid4().hex)
    fd = os.open(name, os.O_CREAT | os.O_EXCL | os.O_WRONLY, creation_mode)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            out.write(text)
            out.flush()
            if mode is not None:
                os.chmod(name, mode)
            os.fsync(out.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


@contextmanager
def write_lock(path):
    # Replacing a page or registry changes its inode. Lock a stable sibling instead.
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as lock:
        if os.name == "nt":
            import msvcrt
            lock.seek(0)
            msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)
        else:
            import fcntl
            fcntl.flock(lock, fcntl.LOCK_EX)
        yield  # closing the handle releases the lock, including on failure


QUESTION_SECTION = "## 질문\n\n"
HISTORY_SECTION = "## 변경 이력\n\n"


def write_page(path, log, q, answer, refs, now, *, creation_mode=0o600):
    """Caller holds the wiki's filing lock. History precedes the current, parseable answer."""
    stamp = (now if now.tzinfo is not None else now.astimezone()).isoformat(timespec="microseconds")
    day = now.strftime("%Y-%m-%d")
    current = (f"{QUESTION_SECTION}{q}\n\n## 답변\n\n{answer}\n\n## 검색 결과\n\n"
               f'{refs or "근거 미기록"}\n')
    history = ""
    mode = None
    if path.exists():
        mode = stat.S_IMODE(path.stat().st_mode)
        previous = path.read_text(encoding="utf-8")
        header, separator, body = previous.partition("\n---\n")
        intro, heading, content = body.partition("\n\n" + QUESTION_SECTION)
        if heading and not content.startswith(q + "\n\n## 답변\n\n"):
            raise ValueError("existing filename belongs to a different question")
        old_current = QUESTION_SECTION + content if heading else previous
        if old_current == current:
            return previous
        if separator and heading:
            _, _, history = intro.partition("\n\n" + HISTORY_SECTION)
        saved = re.search(r"^saved_at: (.+)$", header, re.M)
        created = re.search(r"^created: (.+)$", header, re.M)
        when = saved[1] if saved else f"{created[1] if created else day} (시각 미상)"
        # Quoting old headings keeps parsers that look for the current answer on the current version.
        quoted = "\n".join("> " + line for line in old_current.rstrip("\n").split("\n"))
        if history:
            history = history.rstrip() + "\n\n"
        history += f"### {when}\n\n{quoted}\n\n"
    before_question = f"{HISTORY_SECTION}{history.rstrip()}\n\n" if history else ""
    text = (f'---\ntitle: {json.dumps(q, ensure_ascii=False)}\ntype: query\ntags: [query, {tags(q)}]\ncreated: {day}\nsaved_at: {stamp}\n---'
            f'\n\n{summary(answer)}\n\n{before_question}{current}')
    atomic_text(path, text, mode=mode, creation_mode=creation_mode)
    with log.open("a", encoding="utf-8") as out:
        out.write(f"\n## [{day}] query | {q}\n")
    return text


def save(row, q, a, refs="", now=None):
    """-> (exit code, message). Refused by a save_block rule: 2. Blank answer: 0, nothing written."""
    if not row.get("wiki_root"):
        raise ValueError(f"도서관 {row.get('key')}: 등록부에 wiki_root가 없다")
    hits = [name for name, p in (row.get("save_block") or {}).items() if re.search(p, f"{q}\n{a}\n{refs}", re.I)]
    if hits:
        return 2, f"거부: {row['key']} 도서관 저장 차단 규칙 — 감지: {'|'.join(hits)}"
    if not a.strip():
        return 0, "거부: 답변이 비어 있습니다"
    now = now or datetime.datetime.now()
    root = Path(row["wiki_root"])
    if not root.is_dir():  # a wiki on another machine must not grow a stray copy here
        raise ValueError(f"도서관 {row.get('key')}: wiki_root가 이 기기에 없다 — {root} (그 위키가 있는 기기에서 저장한다)")
    out = root / "queries" / f"{now:%y%m%d}_{slug(q)}.md"
    with write_lock(out.parent / ".filing.lock"):
        write_page(out, root / "log.md", q, a, refs, now, creation_mode=0o666)
    return 0, f"저장: {out}"


def load_row(key):
    r = subprocess.run(["psql", os.environ["DATABASE_URL"], "-Atq", "-v", "ON_ERROR_STOP=1", "-v", f"key={key}"],
                       input=REGISTRY_SQL, capture_output=True, text=True, timeout=30)
    if r.returncode or not r.stdout.strip():
        raise SystemExit(f"도서관 등록부에서 {key}를 못 읽음: {r.stderr.strip()[-300:] or '행 없음(status=active)'}")
    return json.loads(r.stdout)


def main(argv):
    if len(argv) < 4:
        raise SystemExit(__doc__)
    try:
        code, msg = save(load_row(argv[1]), argv[2], argv[3], argv[4] if len(argv) > 4 else "")
    except ValueError as error:
        code, msg = 2, f"거부: {error}"
    print(msg, file=sys.stdout if msg.startswith("저장") else sys.stderr)
    return code


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import search  # noqa: E402  (records the use; see search.traced)
    sys.exit(search.traced("librarian", lambda: main(sys.argv)))
