"""Dependency-free lexical library. The explicit config is the local registry authority.

SIMPLIFY: one JSON index per library keeps BM25 statistics and text in memory per query.
Index refresh scans Markdown; semantic embeddings and incremental indexing are separate adapters.
"""
import collections
import datetime
import importlib.util
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys

# Registry fields the Rust CLI handles; a registry with any other field keeps the policy-aware Python path.
SIMPLE_FIELDS = {"wiki_root", "shelf_root", "central", "name", "status", "reads", "dirs"}
# Rust search records the query when these are set; this compatibility search never recorded.
RECORDING_ENV = ("LIBRARY_DB", "LIBRARY_DB_URL", "LIBRARY_DB_URL_FILE", "LIBRARY_PROJECT", "LIBRARY_ROOM_SOCKET")


def rust_cli():
    """The built Rust CLI, which owns lexical search: BENEDICT_BIN, then PATH, then this checkout's build."""
    if os.environ.get("BENEDICT_BIN"):
        return os.environ["BENEDICT_BIN"]
    if shutil.which("benedict"):
        return shutil.which("benedict")
    for build in ("release", "debug"):
        path = Path(__file__).resolve().parent / "core" / "target" / build / "benedict"
        if path.is_file() and os.access(path, os.X_OK):
            return str(path)
    return None

_spec = importlib.util.spec_from_file_location("_library_filing", Path(__file__).parent / "bin/save_query.py")
_filing = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_filing)


def confined(root, relative):
    path = Path(relative)
    if path.anchor or ".." in path.parts:
        raise ValueError("path must be relative and remain inside the selected library")
    target = (root / path).resolve()
    if not target.is_relative_to(root):
        raise ValueError("symlink escapes selected library")
    return target


INDEX_NOTE = "\n> 개인정보가 들어 있어 본문은 색인하지 않았습니다. 원본 파일을 여세요.\n"
ROOM_KINDS = ("search", "reading", "meeting", "librarian")
SECTION_KINDS = ("today", "meeting_vars", "decisions")
ENV_NAME = re.compile(r"[A-Z_][A-Z0-9_]*")
TABLE_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)?")
COPY_NAME = re.compile(r"[A-Za-z][A-Za-z0-9_]*")
COPY_TAKEN = {"docs", "sync_meta", "library_registry", "ROOM", "open"}   # the sync writes these (and every ROOM_ name)


def check_librarian_choices(key, row):
    """restart (rooms, after), mail_reap (ttl_days, export_dir), journal_db (dsn_env, table): each true/false or a map.
    copies: search-room variable name -> {"file": JSON path} or {"command": command printing JSON}, optional timeout."""
    def number(v):
        return isinstance(v, (int, float)) and not isinstance(v, bool) and v > 0
    checks = {
        "restart": {"rooms": lambda v: isinstance(v, list) and v and all(k in ROOM_KINDS for k in v),
                    "after": lambda v: v in ("clean", "cycle")},
        "mail_reap": {"ttl_days": number, "export_dir": lambda v: isinstance(v, str) and v},
        "journal_db": {"dsn_env": lambda v: isinstance(v, str) and ENV_NAME.fullmatch(v),
                       "table": lambda v: isinstance(v, str) and TABLE_NAME.fullmatch(v)},
    }
    for name, fields in checks.items():
        choice = row.get(name, False)
        if isinstance(choice, bool):
            continue
        if not isinstance(choice, dict) or any(k not in fields or not fields[k](v) for k, v in choice.items()):
            raise ValueError(f"{name} must be true/false or a map of {'/'.join(fields)}: {key}")
    records = row.get("records", False)
    if not isinstance(records, bool) and not (
            isinstance(records, dict) and set(records) <= {"chars"} and all(number(v) and int(v) == v for v in records.values())):
        raise ValueError(f"records must be true/false or {{chars: positive whole number}}: {key}")
    copies = row.get("copies", {})
    taken = COPY_TAKEN | ({"library_terms"} if "terms" in row else set())
    if not isinstance(copies, dict) or any(
            not COPY_NAME.fullmatch(name) or name in taken or name.startswith("ROOM_") or not isinstance(spec, dict)
            or len({"file", "command"} & set(spec)) != 1 or any(k not in ("file", "command", "timeout") for k in spec)
            or not all(isinstance(spec.get(k, "x"), str) and spec.get(k, "x") for k in ("file", "command"))
            or "timeout" in spec and not number(spec["timeout"]) for name, spec in copies.items()):
        raise ValueError(f"copies must map search-room variable names (not the sync's own) to one file or command: {key}")


def index_text(body, rules):
    """Text for search indexes and vectors: a document matching any index_block rule keeps only its front matter
    (or its first line) and a note. The file itself is unchanged and still opens with read."""
    hit = lambda text: any(re.search(rule, text) for rule in rules.values())
    if not rules or not hit(body):
        return body
    if body.startswith("---") and "\n---\n" in body:
        head = body.split("\n---\n", 1)[0] + "\n---\n"
        if not hit(head):
            return head + INDEX_NOTE
    first = body.lstrip("-\n").split("\n", 1)[0]
    return ("" if hit(first) else first) + "\n" + INDEX_NOTE


# Question matching for rank learning: searches use keywords and saves use the asker's question, so they are
# paired by token Jaccard (Korean particles stripped) or short-side coverage, not by exact text.
_JOSA = ('으로', '에서', '에게', '까지', '부터', '처럼', '보다', '한테',
         '은', '는', '이', '가', '을', '를', '의', '에', '로', '와', '과', '도', '만')


RECORD_NOTE = "\n> 서지 레코드만 실었습니다. 원문은 파일을 여세요.\n"
RECORD_CHARS = 400
RECORD_HEAD_MAX = 2000


def record_text(body, chars=RECORD_CHARS):
    """A page's bibliographic record for rooms: its front matter (up to RECORD_HEAD_MAX) and the first `chars` of its
    body. A page that already fits is kept as it is (stripped)."""
    head, rest = "", body
    if body.startswith("---") and "\n---\n" in body:
        head, rest = body.split("\n---\n", 1)
        head = head[:RECORD_HEAD_MAX] + "\n---\n"
    rest = rest.strip()
    return head + (rest[:chars] + RECORD_NOTE if len(rest) > chars else rest)


def _q_norm(s: str) -> str:
    return re.sub(r'\s+', ' ', (s or "").strip().lower())


def _q_toks(s: str) -> set:
    s = re.sub(r'[_\-]+', ' ', (s or "").lower())
    out = set()
    for w in re.findall(r'[0-9a-z가-힣]+', s):
        for j in _JOSA:  # strip a Korean particle (승격은 -> 승격, endpoint의 -> endpoint)
            if len(w) > len(j) + 1 and w.endswith(j) and re.search(r'[가-힣]$', w):
                w = w[:-len(j)]
                break
        out.add(w)
    return out


def _q_score(a: str, b: str) -> float:
    """0..1: 1.0 when equal, else the larger of token Jaccard and short-side coverage."""
    a_n, b_n = _q_norm(a), _q_norm(b)
    if not a_n or not b_n:
        return 0.0
    if a_n == b_n:
        return 1.0
    ta, tb = _q_toks(a_n), _q_toks(b_n)
    if not ta or not tb:
        return 0.0
    inter = len(ta & tb)
    jac = inter / len(ta | tb)
    cov = inter / min(len(ta), len(tb)) if min(len(ta), len(tb)) >= 2 else 0.0
    return max(jac, cov)


def _q_match(qn: str, q: str) -> bool:
    """Same question when token Jaccard >= 0.6 or the shorter side is 80%+ covered."""
    a_n, b_n = _q_norm(qn), _q_norm(q)
    if not a_n:
        return False
    if a_n == b_n:
        return True
    ta, tb = _q_toks(a_n), _q_toks(b_n)
    if not ta or not tb:
        return False
    inter = len(ta & tb)
    if inter / len(ta | tb) >= 0.6:
        return True
    return min(len(ta), len(tb)) >= 2 and inter / min(len(ta), len(tb)) >= 0.8


def learn_from_save(root, question, refs, window_sec=21600, margin=0.05, topk=5, now=None):
    """Rank learning: the saved question's evidence (refs) labels the search that preceded it. Rooms write one
    retrieval log per question (<wiki_root>/.wiki-pending/retrieval, top word and vector hits); here the log
    matching the question is found (exact, else the clearly best match within window_sec), each source gains a
    success when one of its top `topk` hits is cited in refs, else a failure (Beta posterior in rrf_weights.json,
    read by rooms as fusion weights), and the log is consumed. Without the .wiki-pending folder: not configured."""
    pend = Path(root) / ".wiki-pending"
    if not pend.is_dir():
        return "not configured"
    if not refs.strip():
        return "skip(no refs)"
    qn = _q_norm(question)
    log, files = {}, {}
    for f in sorted((pend / "retrieval").glob("*.json")) if (pend / "retrieval").is_dir() else []:
        try:
            entry = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if entry.get("q"):
            log[entry["q"]], files[entry["q"]] = entry, f
    if not log:
        return "skip(no retrieval log)"
    key, entry = qn, log.get(qn)
    if entry is None:
        now = now or datetime.datetime.now(datetime.timezone.utc)
        candidates = []
        for k, v in log.items():
            try:
                at = datetime.datetime.strptime(v.get("ts", ""), "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)
            except ValueError:
                continue
            if (now - at).total_seconds() <= window_sec and _q_match(k, qn):
                candidates.append((_q_score(k, qn), k, v))
        candidates.sort(key=lambda c: -c[0])
        if not candidates:
            return "skip(no matching question)"
        if len(candidates) > 1 and candidates[0][0] - candidates[1][0] < margin:
            return "skip(ambiguous question)"
        _, key, entry = candidates[0]
    cited = refs.lower()
    used = lambda paths: any(Path(p).stem.lower() in cited for p in (paths or [])[:topk] if Path(p).stem)
    weights = pend / "rrf_weights.json"
    try:
        st = json.loads(weights.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        st = {"det": {"a": 1.0, "b": 1.0}, "vec": {"a": 1.0, "b": 1.0}, "n": 0}
    for source in ("det", "vec"):
        if source in entry:
            st.setdefault(source, {"a": 1.0, "b": 1.0})
            st[source]["a" if used(entry[source]) else "b"] += 1.0
    st["n"] = int(st.get("n", 0)) + 1
    st["updated"] = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    weights.write_text(json.dumps(st, ensure_ascii=False), encoding="utf-8")   # in place: network shares break on rename
    files[key].unlink(missing_ok=True)
    mean = lambda s: st[s]["a"] / (st[s]["a"] + st[s]["b"])
    return f"w_det={mean('det'):.3f} w_vec={mean('vec'):.3f} (n={st['n']})"


atomic_text = _filing.atomic_text


KEY = re.compile(r"[a-z0-9][a-z0-9_-]{0,62}")


def check_library_identity(key, name):
    if not KEY.fullmatch(key or "") or not (name or "").strip():
        raise ValueError("key must be lowercase letters, digits, _ or - (up to 63), and name must not be empty")


def init(config, demo=False, key="main", name="Example library"):
    """A new library: its registry (this library central) and an empty wiki — or the fictional demo pages."""
    check_library_identity(key, name)
    config = Path(config).absolute()
    root = config.parent / "wiki"
    if config.exists() or config.is_symlink() or root.exists() or root.is_symlink():
        raise ValueError("init refuses an existing registry or wiki; choose an empty directory")
    config.parent.mkdir(parents=True, exist_ok=True)
    root.mkdir()
    if demo:
        (root / "entities").mkdir()
        (root / "entities/refund.md").write_text(
            "# Synthetic refund policy\nUnused fictional products may be returned within 14 days.\n사용하지 않은 가상 제품은 14일 안에 반품할 수 있습니다.\n", encoding="utf-8")
        (root / "entities/shipping.md").write_text(
            "# Synthetic shipping policy\nFictional parcels leave every Monday.\n", encoding="utf-8")
    registry = {"version": 1, "libraries": {key: {"name": name.strip(), "wiki_root": "wiki", "central": True}}}
    with config.open("x", encoding="utf-8") as out:
        out.write(json.dumps(registry, ensure_ascii=False, indent=2) + "\n")
    Library(config).index()
    return {"config": str(config), "library": key, "demo": demo}


def add(config, key, name=None):
    """Create a local wiki and its index before publishing the central library's new reads entry."""
    name = key if name is None else name.strip()
    check_library_identity(key, name)
    config = Path(config).resolve(strict=True)
    with _filing.write_lock(config.parent / ".library-state" / "registry.lock"):
        lib = Library(config)
        central, central_row, _ = lib.selected()
        libraries = lib.registry["libraries"]
        root = config.parent / f"wiki-{key}"
        new_row = {"name": name, "wiki_root": root.name}
        result = {"config": str(config), "central": central, "library": key, "wiki_root": str(root)}
        if key in libraries:
            if libraries[key] == new_row and key in central_row.get("reads", []) and root.is_dir() and not root.is_symlink():
                return {**result, "created": False}
            raise ValueError(f"library already registered: {key}")
        for row in libraries.values():
            if "wiki_root" in row:
                existing = (config.parent / row["wiki_root"]).resolve()
                if root.is_relative_to(existing) or existing.is_relative_to(root):
                    raise ValueError(f"wiki directory overlaps a registered library: {root}")
        index_file = lib.index_path(key)
        keep_index = index_file.exists()
        # An empty directory may remain if the process stopped before publishing the registry.
        created_root = False
        try:
            root.mkdir()
            created_root = True
        except FileExistsError:
            if root.is_symlink() or not root.is_dir() or any(root.iterdir()):
                raise ValueError(f"new wiki directory must be empty: {root}") from None
        try:
            libraries[key] = new_row
            central_row.setdefault("reads", []).append(key)
            lib.index(key)
            atomic_text(config, json.dumps(lib.registry, ensure_ascii=False, indent=2) + "\n",
                        mode=stat.S_IMODE(config.stat().st_mode))
        except (OSError, ValueError, TypeError):
            if not keep_index:
                index_file.unlink(missing_ok=True)
            if created_root:
                root.rmdir()
            raise
    return {**result, "created": True}


class Library:
    def __init__(self, config):
        self.config = Path(config).resolve(strict=True)
        self.registry = json.loads(self.config.read_text(encoding="utf-8"))
        if not isinstance(self.registry, dict) or self.registry.get("version") != 1 or not isinstance(self.registry.get("libraries"), dict) or not self.registry["libraries"]:
            raise ValueError("registry requires version 1 and nonempty libraries object")
        libraries = self.registry["libraries"]
        for key, row in libraries.items():
            # A library served only by its room elsewhere (room or box) needs no local wiki_root.
            if not isinstance(row, dict) or not (isinstance(row.get("wiki_root"), str) and row["wiki_root"]
                                                 or "wiki_root" not in row and isinstance(row.get("room"), str) and row["room"]):
                raise ValueError(f"invalid library row: {key}")
            if "room" in row and (not isinstance(row["room"], str) or not row["room"]):
                raise ValueError(f"room must be a nonempty room name: {key}")
            if row.get("search", "room") not in ("room", "box"):
                raise ValueError(f"search must be room or box: {key}")
            if "maxd" in row and (isinstance(row["maxd"], bool) or not isinstance(row["maxd"], (int, float)) or not 0 < row["maxd"] <= 2):
                raise ValueError(f"maxd must be a vector distance in (0, 2]: {key}")
            if "central" in row and not isinstance(row["central"], bool):
                raise ValueError(f"central must be boolean: {key}")
            if row.get("status", "active") not in ("active", "closed"):
                raise ValueError(f"status must be active or closed: {key}")
            for field in ("dirs", "cards"):
                values = row.get(field, [])
                if not isinstance(values, list) or any(not isinstance(d, str) or not d or Path(d).anchor or ".." in Path(d).parts for d in values):
                    raise ValueError(f"{field} must be a list of relative paths inside wiki_root: {key}")
            # save_block refuses a save; index_block keeps a matching document's body out of rooms and vectors.
            for field in ("save_block", "index_block"):
                rules = row.get(field, {})
                if not isinstance(rules, dict) or any(not isinstance(p, str) for p in rules.values()):
                    raise ValueError(f"{field} must map names to regex strings: {key}")
                for name, rule in rules.items():
                    try:
                        re.compile(rule)
                    except re.error as error:
                        raise ValueError(f"invalid {field} regex: {key}/{name}") from error
            # sections: side sections of a whole-library search — today (folders), meeting_vars, decisions (sql); terms: a JSON file.
            sections = row.get("sections", {})
            if (not isinstance(sections, dict) or any(k not in SECTION_KINDS or not isinstance(v, dict) for k, v in sections.items())
                    or not isinstance(sections.get("today", {}).get("dirs", []), list)
                    or "decisions" in sections and not isinstance(sections["decisions"].get("sql"), str)):
                raise ValueError(f"sections must map today(dirs)/meeting_vars/decisions(sql) to settings: {key}")
            if "terms" in row and (not isinstance(row["terms"], str) or not row["terms"]):
                raise ValueError(f"terms must be a JSON file path: {key}")
            clean = row.get("clean", False)
            if not isinstance(clean, bool) and not (isinstance(clean, dict) and all(
                    k in ("reading", "meeting", "librarian") and isinstance(v, dict)
                    and all(isinstance(v.get(f, []), list) for f in ("keep", "prefixes", "expire_prefixes")) for k, v in clean.items())):
                raise ValueError(f"clean must be true/false or map reading/meeting/librarian to keep/prefixes rules: {key}")
            check_librarian_choices(key, row)
            rooms = row.get("rooms", {})
            if not isinstance(rooms, dict) or any(k not in ROOM_KINDS or not isinstance(v, str) or not v for k, v in rooms.items()):
                raise ValueError(f"rooms must map {'/'.join(ROOM_KINDS)} to room names: {key}")
        # reads: the other libraries this one may search. One direction only — the central library is read by
        # no one, and a library never reads a library that reads it.
        central = {k for k, r in libraries.items() if r.get("central")}
        for key, row in libraries.items():
            reads = row.get("reads", [])
            if not isinstance(reads, list) or any(r not in libraries or r == key or r in central for r in reads):
                raise ValueError(f"reads must list other registered, non-central libraries: {key}")
            back = [r for r in reads if key in libraries[r].get("reads", [])]
            if back:
                raise ValueError(f"reads must go one way: {key} and {back[0]} read each other")

    def reads(self, key):
        """Rows of the libraries `key` may search, in its reads order."""
        return [(k, self.registry["libraries"][k]) for k in self.registry["libraries"][key].get("reads", [])]

    def selected(self, key=None):
        if key is None:
            central = [k for k, r in self.registry["libraries"].items() if r.get("central")]
            if len(central) != 1:
                raise ValueError("select --library, or configure exactly one central library")
            key = central[0]
        if key not in self.registry["libraries"]:
            raise ValueError(f"unknown library: {key}")
        row = self.registry["libraries"][key]
        if row.get("status", "active") != "active":
            raise ValueError(f"library is not active: {key}")
        if "wiki_root" not in row:
            raise ValueError(f"{key} has no local wiki_root; its room serves it")
        root = (self.config.parent / row["wiki_root"]).resolve(strict=True)
        if not root.is_dir():
            raise ValueError("wiki_root is not a directory")
        return key, row, root

    def document_paths(self, key=None):
        """The library's Markdown files (relative path -> resolved path), without reading them."""
        key, row, root = self.selected(key)
        paths = {}
        found = [p for d in row.get("dirs") or ["."] for p in (root / d).rglob("*.md")]
        for path in sorted(set(found)):
            relative = path.relative_to(root)
            if any(p.startswith(".") for p in relative.parts) or relative.as_posix() == "log.md":
                continue
            resolved = path.resolve()
            if not resolved.is_relative_to(root) or not resolved.is_file():
                continue
            paths[relative.as_posix()] = resolved
        return key, paths

    def index_body(self, key, path):
        """One document's text as search indexes and vectors hold it (index_block applied)."""
        return index_text(Path(path).read_text(encoding="utf-8", errors="replace"),
                          self.registry["libraries"][key].get("index_block", {}))

    def room_body(self, key, path):
        """What the rooms hold for one document: its indexed text, or only its bibliographic record (records)."""
        text = self.index_body(key, path)
        choice = self.registry["libraries"][key].get("records", False)
        if not choice:
            return text
        return record_text(text, (choice if isinstance(choice, dict) else {}).get("chars", RECORD_CHARS))

    def documents(self, key=None):
        key, paths = self.document_paths(key)
        return key, {relative: self.index_body(key, path) for relative, path in paths.items()}

    def index_path(self, key):
        name = hashlib.sha256((str(self.config) + "\0" + key).encode()).hexdigest()[:16] + "-index.json"
        return self.config.parent / ".library-state" / name

    def index(self, key=None):
        key, docs = self.documents(key)
        _, _, root = self.selected(key)
        counts = {p: dict(collections.Counter(re.findall(r"\w+", text.casefold())))
                  for p, text in docs.items()}
        lengths = {p: sum(c.values()) for p, c in counts.items()}
        record = {"version": 1, "root": str(root), "documents": docs, "counts": counts, "lengths": lengths,
                  "indexed_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
        atomic_text(self.index_path(key), json.dumps(record, ensure_ascii=False))
        return {"library": key, "documents": len(docs), "engine": "lexical-bm25", "persistent": True,
                "indexed_at": record["indexed_at"]}

    def refresh(self, key=None, path=None):
        """Re-index; with rooms (LIBRARY_ROOM_HOME) send them what changed, and with vectors (LIBRARY_EMBED_URL and
        LIBRARY_PG_DSN) embed the saved page. A failure is raised, never skipped."""
        result = self.index(key)
        steps = []
        if os.environ.get("LIBRARY_ROOM_HOME"):
            steps.append(("room sync", "room_sync.py", 45))
        if path is not None and os.environ.get("LIBRARY_EMBED_URL") and os.environ.get("LIBRARY_PG_DSN"):
            steps.append(("embedding", "embed_sync.py", 60))
        for what, script, limit in steps:
            args = [sys.executable, str(Path(__file__).parent / "bin" / script), "--config", str(self.config),
                    "--library", result["library"]] + (["--path", str(path)] if path is not None else [])
            synced = subprocess.run(args, capture_output=True, text=True, encoding="utf-8", timeout=limit)
            if synced.returncode:
                raise ValueError(f"page/index written but live {what} failed: " + synced.stderr.strip()[:180])
        return result

    def search(self, query, key=None, top=5):
        result = self.search_one(query, key, top)
        if key is None and self.reads(result["library"]):
            # Without --library the default library also shows its readable libraries' top word hits (no distance limit).
            result["near"] = []
            for other, row in self.reads(result["library"]):
                try:
                    near = {"hits": self.search_one(query, other, 3)["hits"]}
                except ValueError as error:
                    near = {"error": str(error)}
                result["near"].append({"library": other, "name": row.get("name", other), **near})
        return result

    def search_one(self, query, key=None, top=5):
        if top < 1 or top > 100:
            raise ValueError("top must be between 1 and 100")
        words = set(re.findall(r"\w+", query.casefold()))
        if not words:
            raise ValueError("query must contain a word")
        key, _, root = self.selected(key)
        rust = rust_cli()
        if rust and all(set(row) <= SIMPLE_FIELDS for row in self.registry["libraries"].values()):
            return self.rust_search(rust, query, key, top)
        path = self.index_path(key)
        if not path.is_file():
            raise ValueError("prebuilt index missing; run index or librarian before search")
        record = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(record, dict) or record.get("version") != 1 or record.get("root") != str(root):
            raise ValueError("index does not match selected library; rebuild index")
        if not all(isinstance(record.get(k), dict) for k in ("documents", "counts", "lengths")):
            raise ValueError("invalid index; rebuild index")
        docs, counts, lengths = record["documents"], record["counts"], record["lengths"]
        if set(docs) != set(counts) or set(docs) != set(lengths) or not isinstance(record.get("indexed_at"), str):
            raise ValueError("invalid index; rebuild index")
        if any(not isinstance(text, str) for text in docs.values()) or any(not isinstance(c, dict) for c in counts.values()) or any(not isinstance(n, int) or n < 0 for n in lengths.values()):
            raise ValueError("invalid index; rebuild index")
        average = sum(lengths.values()) / max(len(docs), 1) or 1
        scores = collections.defaultdict(float)
        for word in words:
            df = sum(word in c for c in counts.values())
            idf = math.log(1 + (len(docs) - df + .5) / (df + .5))
            for path, counter in counts.items():
                n = counter.get(word, 0)
                if n:
                    scores[path] += idf * n * 2.5 / (n + 1.5 * (.25 + .75 * lengths[path] / average))
        hits = []
        for path, score in sorted(scores.items(), key=lambda pair: (-pair[1], pair[0]))[:top]:
            text = docs[path]
            title = next((line.lstrip("# ") for line in text.splitlines() if line.startswith("# ")), path)
            hits.append({"path": path, "title": title, "score": round(score, 6), "excerpt": text[:500]})
        return {"library": key, "engine": "lexical-bm25", "indexed_at": record["indexed_at"], "hits": hits}

    def rust_search(self, rust, query, key, top):
        """One library's hits from the Rust search, so both clients rank and explain results the same way."""
        env = {k: v for k, v in os.environ.items() if k not in RECORDING_ENV}
        done = subprocess.run([rust, "search", "--config", str(self.config), "--library", key, "--top", str(top), query],
                              capture_output=True, text=True, encoding="utf-8", timeout=60, env=env)
        if done.returncode:
            try:
                message = json.loads(done.stderr)["error"]
            except (ValueError, KeyError, TypeError):
                message = (done.stderr or done.stdout).strip()[:300]
            raise ValueError(message)
        result = json.loads(done.stdout)
        if "fallback" in result:
            # This client searches the prepared index only, as it always did; the Rust CLI may read the files instead.
            raise ValueError(result["fallback"].removesuffix("; searched the wiki files directly"))
        return result

    def read(self, path, key=None):
        key, _, root = self.selected(key)
        target = confined(root, path)
        if target.suffix != ".md":
            raise ValueError("read accepts Markdown documents only")
        raw = target.read_bytes()
        text = raw.decode("utf-8").replace("\r\n", "\n").replace("\r", "\n")
        return {"library": key, "path": str(target.relative_to(root)), "text": text, "sha256": hashlib.sha256(raw).hexdigest()}

    def save(self, question, answer, refs="", key=None):
        key, row, root = self.selected(key)
        if not question.strip():
            raise ValueError("question is empty")
        if not answer.strip():
            raise ValueError("answer is empty")
        hits = [name for name, pattern in row.get("save_block", {}).items()
                if re.search(pattern, f"{question}\n{answer}\n{refs}", re.I)]
        if hits:
            raise ValueError("save blocked by rules: " + ", ".join(hits))
        now = datetime.datetime.now().astimezone()  # the asker's own day, as the legacy writer and wiki names use
        digest = hashlib.sha256(question.encode("utf-8")).hexdigest()[:12]
        relative = f"queries/{now:%y%m%d}_{_filing.slug(question)}_{digest}.md"
        target, log = confined(root, relative), confined(root, "log.md")
        with _filing.write_lock(confined(root, "queries/.filing.lock")):
            text = _filing.write_page(target, log, question, answer, refs, now)
            self.refresh(key, target)
            learning = learn_from_save(root, question, refs)
        return {"library": key, "path": relative, "bytes": len(text.encode()), "saved": True,
                "learning": learning}
