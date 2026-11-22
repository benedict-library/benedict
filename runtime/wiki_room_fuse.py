"""방 검색 하이브리드 — BM25 순위와 벡터(pgvector) 순위를 RRF 로 합친다. 방(23)과 테스트가 같이 쓴다."""

RRF_K = 3


CARD_MARGIN = 0.01


def card_layer(cards, d20, margin=CARD_MARGIN, n=3):
    """서지 레코드 [(경로, 거리)…](거리 오름차순)에서 기준 거리(d20 + margin) 안의 것만 앞에서 n개."""
    return [p for p, d in cards if d <= d20 + margin][:n]


def records_load(rows):
    """DB 행 [(경로, 벡터 글자 '[…]')…] → [(경로, 벡터, 크기)…]."""
    import math
    out = []
    for p, e in rows:
        v = [float(x) for x in e.strip("[]").split(",")]
        out.append((p, v, math.sqrt(sum(x * x for x in v)) or 1.0))
    return out


def record_nearest(qv, recs, n=5):
    """질의 벡터에서 코사인 거리(pgvector <=> 와 같은 값)가 가까운 서지 레코드 n개 [(경로, 거리)…]."""
    import math
    qn = math.sqrt(sum(x * x for x in qv)) or 1.0
    return sorted(((p, 1 - sum(a * b for a, b in zip(qv, v)) / (qn * vn)) for p, v, vn in recs), key=lambda x: x[1])[:n]


SEARCH_QUERY_NAMES = tuple("""Q N MAX_AGE_H REFS_ALPHA meta at age_h _search_room _keys _mf _d _tok _tok_re stamp t0 fresh _nc _nr
    Nd k1 b _vec_note _vec_fut _vec_err _card_box _root _pfx _dsn _likes _chunks_on _card_likes _vector_lookup _vec_pool qt scores
    w df idf p c f det _model vec _qp _vnote _vsec _anchors _qw _tw _idf _jac _gap _bn2p _wd _wv _js _st fused _i _p _cat _pin _link
    top words _read _ENTRY""".split())


def forget_query(g, names=SEARCH_QUERY_NAMES):
    """방 이름공간 g에서 질문 계산 기록을 지운다. → 지운 수"""
    gone = [k for k in names if k in g]
    for k in gone:
        del g[k]
    return len(gone)


def rrf_fuse(det, vec, k=60, w_det=0.5, w_vec=0.5):
    scores = {}
    for rank, d in enumerate(det, 1):
        scores[d] = scores.get(d, 0.0) + w_det / (k + rank)
    for rank, d in enumerate(vec, 1):
        scores[d] = scores.get(d, 0.0) + w_vec / (k + rank)
    det_i = {d: i for i, d in enumerate(det)}
    vec_i = {d: i for i, d in enumerate(vec)}
    return sorted(scores, key=lambda d: (-scores[d], det_i.get(d, 10 ** 9), vec_i.get(d, 10 ** 9)))


def vec_literal(v):
    return "[" + ",".join(f"{x:.8g}" for x in v) + "]"


def retrieval_log(root, q, det, vec, keep=500):
    """Record optional retrieval evidence only when the deployment created its journal folder."""
    import hashlib, json, os, time
    try:
        pend = os.path.join(root, ".wiki-pending")
        if not root or not os.path.isabs(root) or not os.path.isdir(pend):
            return None
        rdir = os.path.join(pend, "retrieval")
        os.makedirs(rdir, exist_ok=True)
        qn = _re.sub(r"\s+", " ", q.strip().lower())
        path = os.path.join(rdir, hashlib.sha1(qn.encode("utf-8")).hexdigest()[:16] + ".json")
        with open(path, "w", encoding="utf-8") as fh:
            json.dump({"q": qn, "det": list(det)[:20], "vec": list(vec)[:20],
                       "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}, fh, ensure_ascii=False)
        names = [n for n in os.listdir(rdir) if n.endswith(".json")]
        if len(names) > keep:  # 최근 keep개만(24 검색기와 같은 상한)
            full = sorted((os.path.join(rdir, n) for n in names), key=os.path.getmtime)
            for old in full[:-keep]:
                try:
                    os.remove(old)
                except OSError:
                    pass
        return path
    except Exception:
        return None


import os as _os
import re as _re

_REFS_RE = _re.compile(r"^## 검색 결과\s*\n+([^\n]+)", _re.M)


def stage1_hit(jac, gap, jac_min=0.15, gap_min=0.025):
    """1단(이미 물어본 질문): 질문↔가장 가까운 질의 페이지 제목 낱말 겹침 또는 1·2위 거리 차이가 확실할 때만."""
    return jac >= jac_min or gap >= gap_min


def stage1_confirmed(page, det, vec):
    """1단 확인: 고정할 질의 페이지가 이번 검색 후보(BM25 상위·벡터 상위)에도 있어야 한다 — 질의 페이지끼리만 가까운 엉뚱한 페이지가 상위를 다 차지하지 않게."""
    return page in det or page in vec


def anchor_first(anchors, ranked):
    seen, out = set(), []
    for p in list(anchors) + list(ranked):
        if p not in seen:
            seen.add(p); out.append(p)
    return out


def refs_of(body):
    m = _REFS_RE.search(body)
    if not m:
        return []
    return [b.strip() for b in m.group(1).split(",") if b.strip().endswith(".md")]


def weighted_jaccard(a, b, idf, default=1.0):
    """낱말 겹침을 드묾(idf)으로 가중 — '연결·역사·논의'처럼 흔한 말만 겹치는 1단 오인을 막는다."""
    if not a or not b:
        return 0.0
    w = lambda s: sum(idf.get(t, default) for t in s)
    return w(a & b) / max(1e-9, w(a | b))


_VER_WORDS = {"최종", "최종본", "수정", "수정본", "추가수정", "보완", "cx수정", "예산변경",
              "pdf", "hwp", "hwpx", "docx", "pptx", "xlsx", "md", "txt"}


def _name_words(s):
    s = _re.sub(r"\.md$", "", s).lower()
    return frozenset(w for w in _re.findall(r"[가-힣A-Za-z0-9]+", s)
                     if w not in _VER_WORDS and not w.isdigit() and len(w) > 1)


def version_key(path):
    """같은 문서의 판(최종·수정·확장자·날짜만 다름)은 같은 열쇠 — 파일 이름 낱말과 부모 폴더 이름 낱말."""
    parts = path.rsplit("/", 2)
    return (_name_words(parts[-1]), _name_words(parts[-2]) if len(parts) > 2 else frozenset())


def collapse_versions(paths):
    """순위를 지키며 같은 문서의 판은 첫 판 한 칸만 남긴다."""
    seen, out = set(), []
    for p in paths:
        k = version_key(p)
        if k in seen:
            continue
        seen.add(k); out.append(p)
    return out


def collapse_room_versions(paths, groups):
    """ROOM_VERSION_GROUP({경로: [묶음, 대표여부]})으로 묶음마다 한 칸.
    대표가 후보에 있으면 대표를, 없으면 그 묶음에서 가장 앞선 후보를 낸다."""
    primary = {}
    for path, pair in groups.items():
        if not isinstance(pair, (list, tuple)) or len(pair) < 2 or not pair[1]:
            continue
        primary.setdefault(pair[0], path)
    members = {}
    for p in paths:
        pair = groups.get(p)
        if isinstance(pair, (list, tuple)) and len(pair) >= 2:
            members.setdefault(pair[0], []).append(p)
    seen, out = set(), []
    for p in paths:
        pair = groups.get(p)
        if not (isinstance(pair, (list, tuple)) and len(pair) >= 2):
            if p not in seen:
                seen.add(p); out.append(p)
            continue
        vg = pair[0]
        if vg in seen:
            continue
        seen.add(vg)
        cands = members[vg]
        pri = primary.get(vg)
        out.append(pri if pri in cands else cands[0])
    return out


def apply_version_collapse(paths, groups=None):
    """변수가 없으면 파일 이름 판 접기. dict(빈 dict 포함)면 열람실 묶음만 쓴다."""
    if not isinstance(groups, dict):
        return collapse_versions(paths)
    return collapse_room_versions(paths, groups)


def chunk_parents(paths, docs, n, sep="::c"):
    """조각 행(원문경로::c번호) 순위를 원문 문서 순위로 — 첫 조각 순서, 방에 없는 문서는 뺀다."""
    seen, out = set(), []
    for p in paths:
        if sep not in p:
            continue
        head, tail = p.rsplit(sep, 1)
        if not tail.isdigit() or head not in docs or head in seen:
            continue
        seen.add(head); out.append(head)
        if len(out) >= n:
            break
    return out


_LEGAL_RE = _re.compile(r"\(주\)|㈜|주식회사|\(유\)|유한회사")


def catalog_name(s):
    """서지 레코드 이름 — 법인 표기·공백·문장부호를 떼고 소문자."""
    return _re.sub(r"[^0-9a-z가-힣]", "", _LEGAL_RE.sub("", s).lower())


def build_catalog(docs, shelf_roots, min_len=2):
    """백과사전의 엔티티 페이지(entities/ 아래, 문서 사본 entities/documents 제외)로 이름 → 페이지 서지 레코드를 만든다.
    이름은 파일 이름에서만 뽑는다 — 제목만 엔티티 이름인 문서형 페이지(공문·명부)는 서지 레코드가 아니다."""
    cat = {}
    for p in docs:
        if "/entities/" not in p or "/entities/documents/" in p or _in_roots(p, shelf_roots):
            continue
        stem = p.rsplit("/", 1)[-1]
        name = catalog_name(stem[:-3] if stem.endswith(".md") else stem)
        if len(name) >= min_len:
            cat.setdefault(name, []).append(p)
    return cat


def catalog_hits(q, catalog):
    """질문 글자 안에 든 서지 레코드 이름의 (고정 페이지, 연결 항목) — 긴 이름 먼저, 겹치면 한 번만.
    서지 레코드 값이 목록이면 그 페이지들이 연결 항목이고, 사전이면 pages(백과사전, 순서대로 고정)·items(근거·사본)다.
    사전 서지 레코드는 도서관을 넘어 이을 수 있다(example 열람실에 스타트업 위키 회사 페이지)."""
    qn = catalog_name(q)
    pin, link = [], []
    for name in sorted((n for n in catalog if len(n) >= 2 and n in qn), key=len, reverse=True):
        card = catalog[name]
        pages, items = (card.get("pages", []), card.get("items", [])) if isinstance(card, dict) else ([], card)
        pin.extend(p for p in pages if p not in pin)
        link.extend(p for p in items if p not in link and p not in pin)
    return pin, link


def _in_roots(p, roots):
    return any(p.startswith(r.rstrip("/") + "/") for r in roots or [])


def layer_order(fused, hits, shelf_roots, pinned=()):
    """서지 레코드 순서: 고정 페이지(서지 레코드 순서) → 연결 항목(융합 순위, 후보에 없던 것은 끝) → 나머지(융합 순위 그대로).
    Unpinned documents retain their relative rank; shelf_roots is kept for adapter compatibility."""
    pos = {p: i for i, p in enumerate(fused)}
    pin = list(dict.fromkeys(pinned))
    top = sorted((h for h in dict.fromkeys(hits) if h not in pin), key=lambda p: pos.get(p, len(fused)))
    head = pin + top
    return head + [p for p in fused if p not in set(head)]


def card_pages(cards):
    """서지 레코드 파일의 고정 페이지 전부 — 다른 도서관 페이지(스타트업 위키 정본 등)를 열람실 범위에 함께 들인다."""
    out = []
    for card in cards.values():
        out.extend(p for p in card.get("pages", []) if p not in out)
    return out


def link_cards(docs, cards):
    """서지 레코드 파일(이름 → pages·items_match·items_exclude·aliases)로 사전 서지 레코드를 만든다.
    pages는 열람실에 있는 것만, items는 열람실 문서 경로가 items_match에 맞고 items_exclude에 안 맞는 것."""
    out = {}
    for name, card in cards.items():
        pages = [p for p in card.get("pages", []) if p in docs]
        inc = _re.compile(card["items_match"]) if card.get("items_match") else None
        exc = _re.compile(card["items_exclude"]) if card.get("items_exclude") else None
        items = [p for p in docs if inc and inc.search(p) and not (exc and exc.search(p)) and p not in pages]
        entry = {"pages": pages, "items": items}
        for n in [name] + list(card.get("aliases", [])):
            out[catalog_name(n)] = entry
    return out


TOK_RE = _re.compile(r"[0-9A-Za-z]+|[가-힣]+")
TOKV = 2
INDEX_KEYS = ("_df", "_tf", "_len", "_head", "_cites", "_sig", "_refs")


TOK_RE_UNDERSCORE = _re.compile(r"[0-9A-Za-z_]+|[가-힣]+")


def tok_py(s, underscore=False):
    out = []
    for w in (TOK_RE_UNDERSCORE if underscore else TOK_RE).findall(s.lower()):
        out.append(w)
        if _re.match(r"[가-힣]", w) and len(w) >= 2:
            out.extend(w[i:i + 2] for i in range(len(w) - 1))
    return out


def _native_lib():
    """The Rust tokenizer (native/, built into the vector image as runtime/liblibrary_native.so; LIBRARY_NATIVE_LIB
    names another file). LIBRARY_NATIVE: auto (default, use it when present) · on (required — missing is an error) ·
    off (Python only)."""
    import ctypes
    mode = _os.environ.get("LIBRARY_NATIVE", "auto")
    if mode == "off":
        return None
    path = _os.environ.get("LIBRARY_NATIVE_LIB") or _os.path.join(_os.path.dirname(_os.path.abspath(__file__)), "liblibrary_native.so")
    if not _os.path.isfile(path):
        if mode == "on":
            raise RuntimeError(f"LIBRARY_NATIVE=on but no native library at {path}")
        return None
    lib = ctypes.CDLL(path)
    lib.library_tok.argtypes = [ctypes.c_char_p, ctypes.c_size_t, ctypes.c_uint8, ctypes.POINTER(ctypes.c_size_t)]
    lib.library_tok.restype = ctypes.c_void_p
    lib.library_free.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
    if hasattr(lib, "library_idx_new"):   # the lexical index (native/src/index.rs); an older library has only the tokenizer
        P, S, Z = ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t
        for name, args, res in (("library_idx_new", [], P), ("library_idx_drop", [P], None),
                                ("library_idx_put", [P, S, Z, S, Z, ctypes.c_uint32], ctypes.c_int32),
                                ("library_idx_remove", [P, S, Z], ctypes.c_int32), ("library_idx_df", [P, S, Z], ctypes.c_uint32),
                                ("library_idx_has", [P, S, Z], ctypes.c_uint8), ("library_idx_count", [P], Z),
                                ("library_idx_paths", [P, ctypes.POINTER(Z)], P),
                                ("library_idx_bm25", [P, S, Z] + [ctypes.c_double] * 4 + [ctypes.POINTER(Z)], P)):
            f = getattr(lib, name)
            f.argtypes, f.restype = args, res
    return lib


_NATIVE = _native_lib()
TOK_ENGINE = "rust" if _NATIVE else "python"


def tok(s, underscore=False):
    """Words of s: runs of [0-9a-z] (with underscore, [0-9a-z_]) or of Hangul syllables, plus two-syllable pieces of
    each Hangul run. The Rust tokenizer gives the same list when present (TOK_ENGINE); text it cannot take (lone
    surrogates) goes to the Python one."""
    if _NATIVE is None:
        return tok_py(s, underscore)
    import ctypes
    try:
        b = s.encode("utf-8")
    except UnicodeEncodeError:
        return tok_py(s, underscore)
    n = ctypes.c_size_t()
    p = _NATIVE.library_tok(b, len(b), 1 if underscore else 0, ctypes.byref(n))
    if not p:
        return tok_py(s, underscore)
    try:
        out = ctypes.string_at(p, n.value).decode("utf-8")
    finally:
        _NATIVE.library_free(p, n.value)
    return out.split("\n") if out else []


def _native_text(fn, *args):
    import ctypes
    n = ctypes.c_size_t()
    ptr = fn(*args, ctypes.byref(n))
    try:
        return ctypes.string_at(ptr, n.value).decode("utf-8") if ptr else ""
    finally:
        if ptr:
            _NATIVE.library_free(ptr, n.value)


class NativeIndex:
    """The room's word postings in Rust (native/src/index.rs) instead of {path: Counter}: put/pop documents, `in`,
    iteration over paths, and bm25() — the same sums rooms/room_query.py makes over the Python dictionaries.
    Index names start with _ so room snapshots never hold it; the next sync rebuilds it after a restart."""

    def __init__(self):
        self._ix = _NATIVE.library_idx_new()

    def __del__(self):
        if getattr(self, "_ix", None) and _NATIVE is not None:
            _NATIVE.library_idx_drop(self._ix)
            self._ix = None

    def put(self, path, counts, length):
        b = path.encode("utf-8")
        d = "".join(f"{w}\t{c}\n" for w, c in counts.items()).encode("utf-8")
        if _NATIVE.library_idx_put(self._ix, b, len(b), d, len(d), length) != 0:
            raise ValueError(f"native index refused {path}")

    def pop(self, path, default=None):
        b = path.encode("utf-8")
        return () if _NATIVE.library_idx_remove(self._ix, b, len(b)) == 1 else (default if default is not None else ())

    def __contains__(self, path):
        b = path.encode("utf-8")
        return bool(_NATIVE.library_idx_has(self._ix, b, len(b)))

    def __len__(self):
        return _NATIVE.library_idx_count(self._ix)

    def __iter__(self):
        out = _native_text(_NATIVE.library_idx_paths, self._ix)
        return iter(out.split("\n") if out else [])

    def df(self, w):
        b = w.encode("utf-8")
        return _NATIVE.library_idx_df(self._ix, b, len(b))

    def bm25(self, words, nd, k1, b, avg):
        """{path: score} for words in the given order (same formula and order as the Python loop)."""
        q = "\n".join(words).encode("utf-8")
        out = _native_text(_NATIVE.library_idx_bm25, self._ix, q, len(q), float(nd), float(k1), float(b), float(avg))
        return {p: float(s) for p, s in (line.rsplit("\t", 1) for line in out.split("\n") if line)}


class NativeDf:
    """_df for a NativeIndex: get(w, 0) and [w] read the word's document count from Rust."""

    def __init__(self, ix):
        self.ix = ix

    def get(self, w, default=0):
        return self.ix.df(w) or default

    def __getitem__(self, w):
        return self.ix.df(w)


def native_index_on():
    """The Rust index is used when the native library has it, unless LIBRARY_NATIVE_INDEX=off."""
    return _NATIVE is not None and hasattr(_NATIVE, "library_idx_new") and _os.environ.get("LIBRARY_NATIVE_INDEX", "on") != "off"


def doc_sig(body):
    return (len(body), hash(body))


def index_parse(p, body):
    """문서 하나 → (낱말 수, 길이, 제목, 인용 문서). 제목 3배·태그 2배, 인용은 질의 페이지의 '검색 결과'만."""
    from collections import Counter
    head = body[:600]
    m = _re.search(r"^title:\s*(.+)$", head, _re.M); title = m.group(1).strip().strip('"') if m else ""
    if not title:  # frontmatter 없는 문서(example 개발 위키 등)는 첫 '# ' 제목을 쓴다
        m = _re.search(r"^#\s+(.+)$", head, _re.M); title = m.group(1).strip() if m else ""
    m2 = _re.search(r"^tags:\s*(.+)$", head, _re.M); tags = m2.group(1) if m2 else ""
    toks = tok(body) + tok(title) * 3 + tok(tags) * 2
    refs = refs_of(body) if "/queries/" in p else []
    return Counter(toks), len(toks), title, refs


def _index_update(g, changed, removed, fresh):
    from collections import Counter
    if fresh:
        if native_index_on():
            ix = NativeIndex()
            g.update(_df=NativeDf(ix), _tf=ix, _len={}, _head={}, _cites=Counter(), _sig={}, _refs={})
        else:
            g.update(_df=Counter(), _tf={}, _len={}, _head={}, _cites=Counter(), _sig={}, _refs={})
    _df, _tf, _len, _head, _cites, _sig, _refs = (g[k] for k in INDEX_KEYS)
    native = hasattr(_tf, "put")   # by behaviour — a room can hold the index made by another load of this module
    for p in list(removed) + list(changed):
        for w in _tf.pop(p, {}): _df[w] -= 1   # a NativeIndex keeps its own counts (pop gives nothing back)
        for bn in _refs.pop(p, []): _cites[bn] -= 1
        _len.pop(p, None); _head.pop(p, None); _sig.pop(p, None)
    for p, body in changed.items():
        c, n, title, refs = index_parse(p, body)
        if native:
            _tf.put(p, c, n)
        else:
            _tf[p] = c
            for w in c: _df[w] += 1
        _len[p] = n; _head[p] = title; _refs[p] = refs; _sig[p] = doc_sig(body)
        for bn in refs: _cites[bn] += 1
    g.update(_tokv=TOKV, _avg=sum(_len.values()) / max(1, len(_len)))


def index_refresh(g, docs):
    """열람실: 방 안 docs 와 색인을 대조해 바뀐 문서만 빼고 더한다(증분). 토큰 규칙이 바뀌면 한 번 전량. → (전량?, 바뀐 수, 빠진 수)"""
    fresh = "_tf" not in g or "_sig" not in g or g.get("_tokv") != TOKV
    sig = {} if fresh else g["_sig"]
    removed = [] if fresh else [p for p in list(g["_tf"]) if p not in docs]
    changed = {p: b for p, b in docs.items() if sig.get(p) != doc_sig(b)}
    _index_update(g, changed, removed, fresh)
    return fresh, len(changed), len(removed)


def index_apply(g, changed, removed):
    """검색실: 사서가 보낸 변경분(경로 → 본문)과 빠진 경로로 색인을 고친다. 본문은 색인만 만들고 남기지 않는다."""
    fresh = "_tf" not in g or g.get("_tokv") != TOKV
    _index_update(g, changed, removed, fresh)
    return len(g["_len"])


def index_prior(cites, keys, alpha):
    """REFS 사전확률 — 자라기 페이지가 '검색 결과'로 인용한 횟수만큼 곱한다."""
    import math
    return {p: 1.0 + alpha * math.log1p(max(0, cites.get(p.rsplit("/", 1)[-1], 0))) for p in keys}


def read_lines(docs, paths, words, heads):
    """문서마다 제목 줄 번호와 질문 낱말이 든 줄 두 개(검색 결과 줄)."""
    out = []
    for p in paths:
        lines = docs[p].split("\n")
        tl = next((i + 1 for i, l in enumerate(lines) if l.startswith("title:")), 2)
        out.append(f"{p}:{tl}:title: {heads.get(p, '')}")
        shown = 0
        for i, l in enumerate(lines):
            if l.startswith("title:") or l.startswith("---") or not l.strip():
                continue
            if any(w in l.lower() for w in words):
                out.append(f"{p}:{i + 1}:{l.strip()[:160]}"); shown += 1
                if shown >= 2:
                    break
    return "\n".join(out) + ("\n" if out else "")


def excerpts(docs, paths, words, exc):
    """발췌: 문서마다 질문 낱말이 든 줄과 앞뒤 한 줄(문서당 최대 exc자)."""
    out = []
    for p in paths:
        ls = docs[p].split("\n"); keep = []; size = 0
        for i, l in enumerate(ls):
            if l.startswith(("title:", "tags:", "---")) or not l.strip() or not any(w in l.lower() for w in words):
                continue
            for j in range(max(0, i - 1), min(len(ls), i + 2)):
                if j not in keep and ls[j].strip() and not ls[j].startswith("---"):
                    keep.append(j); size += len(ls[j])
            if size > exc:
                break
        out.append("## 발췌 " + p + "\n" + "\n".join(ls[j] for j in sorted(keep))[:exc])
    return "\n".join(out) + ("\n" if out else "")


from pathlib import Path as _Path
READ_PRELUDE = ("import sys as _s\n"
                f"_s.path.insert(0, {str(_Path(__file__).resolve().parent)!r})\n"
                "import wiki_room_fuse as _fz\n")


def room_call(room, code, timeout=20):
    """Execute in another trusted local room under the explicitly configured runtime root."""
    import json, os
    import room_channel
    if not _re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,47}", room):
        raise ValueError("invalid room name")
    root = os.environ.get("LIBRARY_ROOM_HOME", "")
    if not root or not os.path.isabs(root):
        raise ValueError("set LIBRARY_ROOM_HOME explicitly")
    sock = os.path.join(root, room + ".sock")
    with room_channel.connect(sock, timeout + 5) as connection:
        with connection.makefile("rwb") as stream:
            stream.write((json.dumps({"cmd": "exec", "code": code, "timeout": timeout}) + "\n").encode())
            stream.flush()
            line = stream.readline()
    result = json.loads(line) if line else {"ok": False, "out": "room closed"}
    if not result.get("ok"):
        raise RuntimeError(f"room execution failed: {result.get('out', '')[:200]}")
    return result["out"]


# Side sections of a whole-library search (_ENTRY): each is a registry choice (`sections`), copied by room_sync into the
# search room variable ROOM_SECTIONS. A section not chosen says so; a failing one says why; the others still print.
SECTION_TITLES = {"today": "오늘 들어온 글", "meeting_vars": "회의실 변수", "decisions": "결정 기록"}
NOT_CHOSEN = "(설정 안 됨)"
MEETING_VARS_CODE = """def _library_meeting_vars(Q, N, g):
    import re, json, math
    try:  # the room's library module (Rust tokenizer when built); a room without it tokenizes here
        from wiki_room_fuse import tok as _ftok
        _ftok('a_b', underscore=True)
        def _tok(s):
            return set(_ftok(s, underscore=True))
    except (ImportError, TypeError):
        _tok_re = re.compile(r'[0-9A-Za-z_]+|[가-힣]+')
        def _tok(s):
            out = set()
            for w in _tok_re.findall(s.lower()):
                out.add(w)
                if len(w) >= 2 and '가' <= w[0] <= '힣':
                    out.update(w[i:i+2] for i in range(len(w)-1))
            return out
    qt = {w for w in _tok(Q) if len(w) >= 2}
    rows = []
    for k, v in list(g.items()):
        if k.startswith('_') or callable(v) or type(v).__name__ == 'module':
            continue
        try:
            title = (v.get('title') or v.get('summary') or '') if isinstance(v, dict) else ''
            s = json.dumps(v, ensure_ascii=False)[:3000] if isinstance(v, (dict, list)) else str(v)[:3000]
        except Exception:
            continue
        ov = qt & _tok(s); ovn = qt & _tok(k.replace('_', ' ')); ovt = qt & _tok(str(title))
        if ov or ovn:
            rows.append((round(len(ov) / math.log(len(s) + 10) + 2 * len(ovn) + 1.5 * len(ovt), 2), k, type(v).__name__, str(title)[:60]))
    rows.sort(key=lambda r: -r[0])
    for sc, k, ty, title in rows[:N]:
        print(f"{k} ({ty}) {title}")
try:
    _library_meeting_vars(__QUERY__, __TOPN__, globals())
finally:
    del _library_meeting_vars
"""


_TODAY_LISTS = {}  # folder -> (mtime_ns, names): listed again only when the folder's mtime moves


def _folder_names(d):
    """os.listdir(d), reused while d's mtime is unchanged — a shelf folder of ~10,000 pages takes 0.4s to list."""
    m = _os.stat(d).st_mtime_ns
    got = _TODAY_LISTS.get(d)
    if got is None or got[0] != m:
        got = _TODAY_LISTS[d] = (m, _os.listdir(d))
    return got[1]


def entry_today(q, cfg, ymd=None, scan=40, n=8):
    """Pages whose names start with today's date (YYMMDD or NN_YYMMDD), newest first per folder, keeping those
    holding a question word (the 8 longest); at most n."""
    import datetime, os
    ymd = ymd or datetime.date.today().strftime("%y%m%d")
    dated = _re.compile(rf"^([0-9]+_)?{ymd}")
    names = []
    for d in cfg.get("dirs", []):
        try:
            hit = [x for x in _folder_names(d) if dated.match(x)]
        except OSError:
            continue
        def _age(x, d=d):
            try:
                return -os.path.getmtime(os.path.join(d, x))
            except OSError:
                return 0
        names += sorted(hit, key=_age)
    ws = sorted({w.lower() for w in _re.findall(r"[가-힣]{2,}|[A-Za-z0-9]{2,}", q)}, key=len, reverse=True)[:8]
    return "\n".join([x for x in names[:scan] if not ws or any(w in x.lower() for w in ws)][:n])


def entry_meeting_vars(q, cfg, call=None):
    """Meeting-room variables sharing question words, by name and title weight (top cfg n, default 5)."""
    code = MEETING_VARS_CODE.replace("__QUERY__", repr(q)).replace("__TOPN__", str(int(cfg.get("n", 5))))
    out = (call or room_call)(cfg["room"], code)
    return "\n".join([l for l in out.splitlines() if l.strip() and not l.startswith(("Traceback", "  File"))][-5:])


def entry_decisions(qv, cfg, connect=None):
    """Decision records nearest the question vector: cfg sql (with %(vec)s and %(k)s) on LIBRARY_PG_DSN; one line per
    row, columns joined by ' | ' (NULL as empty), at most cfg k (default 3)."""
    import os
    if not qv:
        return "(질의 벡터 없음 — 건너뜀)"
    k = int(cfg.get("k", 3))
    if connect is None:
        import psycopg2
        connect = lambda: psycopg2.connect(os.environ["LIBRARY_PG_DSN"], connect_timeout=3)
    conn = connect()
    try:
        with conn, conn.cursor() as cur:
            cur.execute(cfg["sql"], {"vec": "[" + ",".join(f"{x:.6f}" for x in qv) + "]", "k": k})
            rows = cur.fetchall()
    finally:
        conn.close()
    return "\n".join(" | ".join("" if v is None else str(v) for v in r)[:140] for r in rows[:k])


def entry_sections(query, vector, sections=None):
    """Side sections in registry order today · meeting variables · decisions, each under its '## title'."""
    import concurrent.futures as cf
    sections = sections or {}
    run = {"today": lambda c: entry_today(query, c), "meeting_vars": lambda c: entry_meeting_vars(query, c),
           "decisions": lambda c: entry_decisions(vector, c)}
    with cf.ThreadPoolExecutor(3) as ex:
        futs = {name: ex.submit(run[name], sections[name]) for name in SECTION_TITLES if name in sections}
    out = []
    for name, default in SECTION_TITLES.items():
        cfg = sections.get(name)
        if cfg is None:
            body = NOT_CHOSEN
        else:
            try:
                body = futs[name].result()
            except Exception as e:
                body = f"(건너뜀: {type(e).__name__}: {' '.join(str(e).split())[:80]})"
        out.append(f"## {(cfg or {}).get('title', default)}" + (f"\n{body}" if body else ""))
    return "\n".join(out) + "\n"
