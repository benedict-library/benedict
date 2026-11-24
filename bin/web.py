#!/usr/bin/env python3
"""Search the web for one question, keep the originals on the library's shelf, file the answer in its cabinet.

usage: web.py <library key> "question"

`benedict search --web` runs this through LIBRARY_WEB_CMD on the machine that holds the shelf. Only the
question leaves the library, and it is refused first if it looks like a secret, an inside address or a pasted document.

Registry row (library.registry through DATABASE_URL, column options):
  web_shelf     folder for this library's originals, inside its information boundary. Without it the search is refused.
  web_save_cmd  this library's cabinet writer; question and answer are appended. Default: bin/save_query.py.
  web_block     {name: regex}: more questions to refuse, on top of the built-in rules and save_block.
Settings (environment, else ~/.config/library/search.env):
  LIBRARY_WEB_SEARCH_CMD      the search means; the question is appended. It prints JSON
                              {"summary": text, "sources": [{"url": ..., "note": ...}]}
  LIBRARY_WEB_SEARCH_TIMEOUT  seconds for the means (default 360)

On the shelf, <shelf>/<yymmdd>/<sha256>.<ext> holds each response body as received and, for HTML, a .txt with its text
alone. <shelf>/<yymmdd>.jsonl gets one line per search: question, time, summary, page, and for each URL the address,
time, status, type, size, hash and paths, or why the original was not received. Nothing on the shelf is replaced.
The same question on the same day returns the first result without searching again.

Exit: 0 filed; 2 refused, nothing left the library; 3 the means failed; 4 the cabinet refused the answer.
"""
import concurrent.futures
import datetime
import hashlib
import html.parser
import http.client
import ipaddress
import json
import re
import socket
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import save_query  # noqa: E402
import common  # noqa: E402

MAX_QUESTION = 300               # a longer question is likely a pasted document
MAX_SOURCES = 8
MAX_BYTES = 20 * 1024 * 1024     # one original
FETCH_SOCKET_TIMEOUT = 15
FETCH_TOTAL_TIMEOUT = 40         # one original, start to end
THIN_TEXT = 200                  # fewer letters on an HTML page: a script shell or a block page, not the article
SAVE_TIMEOUT = 60
USER_AGENT = "Mozilla/5.0 (compatible; library-web/1.0)"
OUTBOUND = {
    "비밀값 모양": r"\b(?:sk|pk|ghp|gho|glpat|xox[abpr])[-_][A-Za-z0-9_-]{10,}|\bAKIA[A-Z0-9]{12,}|\beyJ[\w-]{10,}\.[\w-]{10,}"
                  r"|\b(?:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*\S+",
    "긴 무작위 값": r"\b[A-Fa-f0-9]{32,}\b|\b[A-Za-z0-9+_-]{40,}={0,2}(?![\w/])",
    "접속 주소": r"\b(?:postgres(?:ql)?|mysql|redis|mongodb|amqp)(?:\+\w+)?://|\b\w+://[^\s/@]+:[^\s/@]+@",
    "내부 주소": r"\b(?:10\.\d{1,3}\.\d{1,3}\.\d{1,3}|192\.168\.\d{1,3}\.\d{1,3}|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}"
                r"|127\.\d{1,3}\.\d{1,3}\.\d{1,3}|localhost)\b",
    "파일 경로": r"(?:^|[\s\"'(])(?:~|/(?:home|mnt|root|etc|srv|var|opt|tmp|usr|Users))/|\b[A-Za-z]:\\",
    "이메일": r"[\w.+-]+@[\w-]+\.[\w.-]+",
}
EXT = {"text/html": ".html", "application/xhtml+xml": ".html", "application/pdf": ".pdf", "text/plain": ".txt",
       "text/markdown": ".md", "text/csv": ".csv", "application/json": ".json", "application/xml": ".xml", "text/xml": ".xml"}


def outbound_block(q, row):
    """Why this question must stay inside, or None."""
    if len(q) > MAX_QUESTION:
        return f"질문 {len(q)}자 > {MAX_QUESTION}자 — 문서를 붙인 질문으로 본다"
    options = row.get("options") or {}
    rules = {**OUTBOUND, **(row.get("save_block") or {}), **(options.get("web_block") or {})}
    hits = [name for name, p in rules.items() if re.search(p, q, re.I)]
    return "감지: " + "|".join(hits) if hits else None


def why_not_fetch(url):
    """Why this address must not be opened, or None. Addresses come from outside and the fetch runs inside."""
    try:
        p = urllib.parse.urlsplit(url)
        host, port = p.hostname, p.port
    except ValueError as e:
        return f"주소 해석 실패: {e}"
    if p.scheme not in ("http", "https") or not host:
        return "http(s) 주소 아님"
    if p.username or p.password:
        return "주소에 계정이 있음"
    try:
        public_endpoints(host, port or (443 if p.scheme == "https" else 80))
    except (OSError, ValueError) as e:
        return str(e)
    return None


def public_endpoints(host, port):
    infos = socket.getaddrinfo(host, port, type=socket.SOCK_STREAM, proto=socket.IPPROTO_TCP)
    for _, _, _, _, addr in infos:
        ip = ipaddress.ip_address(addr[0].split("%")[0])
        ip = getattr(ip, "ipv4_mapped", None) or ip
        if not ip.is_global or ip.is_multicast:
            raise ValueError("내부 주소 또는 비공개 대상")
    if not infos:
        raise OSError("no public endpoint")
    return infos


def connect_public(address, timeout, source_address=None):
    # Connect to a checked numeric endpoint; keep the hostname for TLS verification.
    last_error = None
    for family, kind, protocol, _, endpoint in public_endpoints(*address):
        sock = socket.socket(family, kind, protocol)
        try:
            sock.settimeout(timeout)
            if source_address:
                sock.bind(source_address)
            sock.connect(endpoint)
            return sock
        except OSError as exc:
            sock.close()
            last_error = exc
    raise last_error


def public_connection(kind, *args, **kwargs):
    connection = kind(*args, **kwargs)
    connection._create_connection = connect_public
    return connection


class PublicHTTP(urllib.request.HTTPHandler):
    def http_open(self, req):
        from functools import partial
        return self.do_open(partial(public_connection, http.client.HTTPConnection), req)


class PublicHTTPS(urllib.request.HTTPSHandler):
    def https_open(self, req):
        from functools import partial
        return self.do_open(partial(public_connection, http.client.HTTPSConnection), req,
                            context=self._context)


class GuardedRedirect(urllib.request.HTTPRedirectHandler):
    def __init__(self, guard):
        self.guard = guard

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        why = self.guard(newurl)
        if why:
            raise urllib.error.URLError(f"넘겨준 주소 막음: {why}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class PageText(html.parser.HTMLParser):
    SKIP = {"script", "style", "noscript", "template", "svg"}
    BLOCK = {"p", "div", "br", "li", "tr", "td", "th", "h1", "h2", "h3", "h4", "h5", "h6", "section", "article", "title",
             "header", "footer", "pre", "blockquote", "ul", "ol", "table"}

    def __init__(self):
        super().__init__()
        self.parts, self.skipping = [], 0

    def handle_starttag(self, tag, attrs):
        self.skipping += tag in self.SKIP
        if tag in self.BLOCK:
            self.parts.append("\n")

    def handle_endtag(self, tag):
        self.skipping -= tag in self.SKIP and self.skipping > 0
        if tag in self.BLOCK:
            self.parts.append("\n")

    def handle_data(self, data):
        if not self.skipping:
            self.parts.append(data)


def decode(data, ctype):
    m = re.search(r"charset=([\w-]+)", ctype, re.I) or re.search(r"charset=[\"']?([\w-]+)", data[:4096].decode("latin-1"), re.I)
    try:
        return data.decode(m.group(1) if m else "utf-8", "replace")
    except LookupError:
        return data.decode("utf-8", "replace")


def page_text(data, ctype):
    parser = PageText()
    parser.feed(decode(data, ctype))
    lines = (re.sub(r"[ \t\r\f\v]+", " ", line).strip() for line in "".join(parser.parts).split("\n"))
    return "\n".join(line for line in lines if line) + "\n"


def keep(path, data):
    """Write once: a file with this hash already holds these bytes, and shelf files are not replaced."""
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with open(path, "xb") as f:
            f.write(data)
    except FileExistsError:
        pass
    if path.stat().st_size != len(data):
        raise OSError(f"서가 파일 크기 불일치: {path}")


def receive(url, guard):
    """(status, final url, content type, body). Raises on anything that is not a whole body."""
    why = guard(url)
    if why:
        raise urllib.error.URLError(why)
    handlers = [GuardedRedirect(guard)]
    if guard is why_not_fetch:
        handlers.extend([urllib.request.ProxyHandler({}), PublicHTTP(), PublicHTTPS()])
    opener = urllib.request.build_opener(*handlers)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "*/*"})
    deadline = time.monotonic() + FETCH_TOTAL_TIMEOUT
    with opener.open(request, timeout=FETCH_SOCKET_TIMEOUT) as r:
        chunks, size = [], 0
        while True:
            chunk = r.read1(65536)
            if not chunk:
                break
            size += len(chunk)
            if size > MAX_BYTES:
                raise ValueError(f"{MAX_BYTES}B 넘음")
            if time.monotonic() > deadline:
                raise TimeoutError(f"{FETCH_TOTAL_TIMEOUT}초 넘음")
            chunks.append(chunk)
        return r.status, r.geturl(), r.headers.get("Content-Type", ""), b"".join(chunks)


def shelve(source, folder, guard, clock):
    """One URL -> its shelf record. A URL not received says why instead of having a hash."""
    rec = {"url": source["url"], "note": source.get("note", ""), "fetched_at": clock().isoformat(timespec="seconds")}
    try:
        status, final, ctype, body = receive(source["url"], guard)
        if not body:
            return {**rec, "status": status, "error": "빈 본문"}
        digest = hashlib.sha256(body).hexdigest()
        kind = ctype.split(";")[0].strip().lower()
        path = folder / (digest + EXT.get(kind, ".bin"))
        keep(path, body)
        text_path, thin = None, False
        if EXT.get(kind) == ".html":
            text = page_text(body, ctype)
            text_path = folder / (digest + ".txt")
            keep(text_path, text.encode("utf-8"))
            thin = len(text) < THIN_TEXT
        elif kind.startswith("text/") or kind in ("application/json", "application/xml"):
            text_path = path
        return {**rec, "final_url": final, "status": status, "content_type": ctype, "bytes": len(body), "sha256": digest,
                "path": str(path), "text_path": str(text_path) if text_path else None, "thin": thin}
    except urllib.error.HTTPError as e:
        e.close()
        return {**rec, "status": e.code, "error": f"HTTP {e.code}"}
    except (urllib.error.URLError, http.client.HTTPException, OSError, ValueError) as e:
        return {**rec, "error": str(getattr(e, "reason", e))[:200] or type(e).__name__}


def fetch_agent_original(row, question, url, shelf):
    """Apply the shared outbound policy and stage an agent-selected original."""
    question = question.strip()
    if not question:
        raise ValueError("provide the public search question")
    reason = outbound_block(question, row)
    if reason:
        raise ValueError(reason)
    # Check the URL separately: long public URLs are legitimate but credentials are not.
    if any(re.search(pattern, url, re.I) for pattern in OUTBOUND.values()):
        raise ValueError("source URL contains private information")
    if any(re.search(pattern, url, re.I) for pattern in (row.get("save_block") or {}).values()):
        raise ValueError("source URL blocked by destination rules")
    now = datetime.datetime.now().astimezone()
    record = shelve({"url": url}, shelf, why_not_fetch, lambda: now)
    if "sha256" not in record:
        raise ValueError("could not archive the original: " + record["error"])
    return record


def agent_source(lib, key, question, url):
    """Archive an agent-selected original using only the caller's local JSON registry."""
    from library_local import confined

    key, row, root = lib.selected(key)
    question = question.strip()
    record = fetch_agent_original(row, question, url, confined(root, ".web-shelf"))
    original = Path(record["path"]).relative_to(root)
    readable = Path(record["text_path"] or record["path"]).relative_to(root)
    description = (f"Source: {url}\nFinal URL: {record['final_url']}\nFetched: {record['fetched_at']}"
                   f"\nHTTP: {record['status']}\nSHA-256: {record['sha256']}\nOriginal: {original.as_posix()}"
                   f"\nRead: {readable.as_posix()}")
    if record["thin"]:
        description += "\nThe extracted text is short; check that it contains the requested evidence."
    saved = lib.save(f"{question} [{url}]", description, url, key)
    return {**saved, "original": str(root / original), "read_path": str(root / readable),
            "sha256": record["sha256"], "thin": record["thin"]}


def ask_means(cfg, q):
    """-> ({"summary", "sources"}, None) or (None, why). Only the question goes to the means."""
    cmd = cfg.get("LIBRARY_WEB_SEARCH_CMD")
    rc, out, err = common.run(common.split_cmd(cmd) + [q], "", int(cfg.get("LIBRARY_WEB_SEARCH_TIMEOUT", 360)))
    if rc:
        return None, f"rc={rc} {(err or out).strip()[:200]}"
    try:
        found = json.loads(out)
        summary, sources = found["summary"].strip(), found["sources"]
    except (ValueError, KeyError, TypeError, AttributeError):
        return None, f"형식 어긋남: {out.strip()[:200]}"
    seen, picked = set(), []
    for s in sources if isinstance(sources, list) else []:
        url = s.get("url", "") if isinstance(s, dict) else ""
        if re.match(r"https?://", url) and url not in seen:
            seen.add(url)
            picked.append({"url": url, "note": s.get("note") or ""})
    if not summary or not picked:
        return None, f"요약이나 출처 URL 없음: {out.strip()[:200]}"
    return {"summary": summary, "sources": picked[:MAX_SOURCES]}, None


def source_line(r):
    if "sha256" not in r:
        return f"- {r['url']} {r['note']} — 원문 못 받음: {r['error']}"
    thin = " (글자 적음 — 스크립트·차단 화면일 수 있음)" if r["thin"] else ""
    return f"- {r['url']} {r['note']} — 서가 {r['text_path'] or r['path']} sha256 {r['sha256'][:12]} {r['fetched_at']} 수집{thin}"


def compose(summary, sources, day, record_file):
    """The cabinet answer: the means' summary first (the page's lead line), then what it is and where each original is."""
    return "\n".join([summary, "", f"웹 검색 {day} 바깥 자료 — 위 요약은 검색 수단의 말이고 원문과 대조하기 전이다. 근거로 쓰려면 아래 서가 원문을 읽는다.",
                      "", "출처:"] + [source_line(r) for r in sources] + ["", f"서가 기록: {record_file}"])


def report(rec, record_file):
    got = sum("sha256" in r for r in rec["sources"])
    print("\n".join([f"요약: {rec['summary']}"] + [source_line(r) for r in rec["sources"]]
                    + [f"원문 {got}/{len(rec['sources'])}건 받음", f"서가 기록: {record_file}", f"서류철: {rec['page']}"]))


def file_answer(row, q, answer):
    """-> (rc, writer's message). The library's own writer when the registry names one; it keeps its index current."""
    cmd = (row.get("options") or {}).get("web_save_cmd")
    if not cmd:
        try:
            return save_query.save(row, q, answer)
        except ValueError as error:  # e.g. the wiki lives on another machine: name a web_save_cmd for it
            return 2, f"거부: {error}"
    # No refs: nobody judged the inside hits on this path, and a refs line would teach rank learning they failed.
    rc, out, err = common.run(common.split_cmd(cmd) + [q, answer], "", SAVE_TIMEOUT)
    return rc, (out + err).strip()


def saved_path(msg):
    """The writer's page: the legacy '저장: <path>' line, or bin/library.py's JSON line {"saved": true, "path": ...}."""
    m = re.search(r"저장: (\S+)", msg or "")
    if m:
        return m.group(1)
    for line in (msg or "").splitlines():
        try:
            d = json.loads(line)
        except ValueError:
            continue
        if isinstance(d, dict) and d.get("saved") is True and isinstance(d.get("path"), str):
            return d["path"]
    return None


def earlier(record_file, q):
    """Today's successful search for this exact question, if any."""
    if not record_file.exists():
        return None
    rows = [json.loads(line) for line in record_file.read_text(encoding="utf-8").splitlines() if line.strip()]
    return next((r for r in reversed(rows) if r.get("question") == q and r.get("sources")), None)


def append(record_file, rec):
    record_file.parent.mkdir(parents=True, exist_ok=True)
    with open(record_file, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")


def main(argv, guard=why_not_fetch, clock=datetime.datetime.now):
    if len(argv) != 3:
        raise SystemExit(__doc__)
    key, q = argv[1], argv[2].strip()
    now = clock()
    cfg, row = common.settings(), save_query.load_row(key)
    shelf = (row.get("options") or {}).get("web_shelf")
    if not shelf:
        print(f"웹 검색 안 함: {key} 도서관은 등록부에 서가 자리(options.web_shelf)가 없다 — 바깥으로 아무것도 보내지 않았다")
        return 2
    why = outbound_block(q, row)
    if why:
        print(f"웹 검색 안 함: 바깥으로 보내지 않음({why}) — 공개된 말로 질문을 다시 쓴다")
        return 2
    if not cfg.get("LIBRARY_WEB_SEARCH_CMD"):
        print("웹 검색 안 함: 이 기기에 LIBRARY_WEB_SEARCH_CMD 설정이 없다")
        return 2
    day = f"{now:%y%m%d}"
    record_file = Path(shelf) / f"{day}.jsonl"
    prior = earlier(record_file, q)
    if prior:
        print(f"오늘 이미 찾음({prior['at']}) — 다시 나가지 않았다")
        report(prior, record_file)
        return 0
    at = now.isoformat(timespec="seconds")
    found, failed = ask_means(cfg, q)
    if failed:
        append(record_file, {"question": q, "at": at, "error": failed})
        print(f"웹 검색 수단 실패: {failed}")
        return 3
    folder = Path(shelf) / day
    with concurrent.futures.ThreadPoolExecutor(max_workers=MAX_SOURCES) as pool:
        sources = list(pool.map(lambda s: shelve(s, folder, guard, clock), found["sources"]))
    rc, msg = file_answer(row, q, compose(found["summary"], sources, day, record_file))
    saved = saved_path(msg)
    rec = {"question": q, "at": at, "summary": found["summary"], "page": saved, "sources": sources}
    append(record_file, rec)
    report(rec, record_file)
    if rc or not saved:
        print(f"서류철 저장 안 됨: {msg}")
        return 4
    return 0


def native_fetch(argv):
    """Private transport for the Rust receipt writer. Reuse the same registry and URL checks."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
    from library_local import Library
    if len(argv) != 5:
        raise ValueError("fetch needs registry, library, staging directory, public query, and URL")
    config, key, staging, query, url = argv
    _, row, _ = Library(config).selected(key)
    record = fetch_agent_original(row, query, url, Path(staging))
    print(json.dumps(record, ensure_ascii=False))


if __name__ == "__main__":
    if sys.argv[1:2] == ["--agent-fetch"]:
        try:
            native_fetch(sys.argv[2:])
        except (ValueError, OSError, TypeError) as error:
            print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
            sys.exit(2)
    else:
        sys.exit(common.traced("search", lambda: main(sys.argv)))
