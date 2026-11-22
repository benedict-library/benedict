# 방 검색 코드 — bin/search.py 가 채워 방(메모리)에 exec 로 보낸다. 방 안에 함수·색인을 캐시한다.
# 채우는 자리: __QUERY__ __TOPN__ __MAXAGE__ __REFSALPHA__ __ROOMLIB__(방이 도는 기기의 방 모듈 폴더 목록) __DSNFILE__(벡터 DB 접속 값이 든 파일).
import re, math, time, datetime, collections
Q = __QUERY__
N = __TOPN__
MAX_AGE_H = __MAXAGE__
REFS_ALPHA = __REFSALPHA__
meta = globals().get('sync_meta') or {}
at = meta.get('at')
age_h = None
if at:
    age_h = (datetime.datetime.now() - datetime.datetime.fromisoformat(at)).total_seconds()/3600
# 검색실(큰 도서관)은 본문(docs) 없이 사서가 넣은 색인만 든다 — 본문이 필요한 줄·발췌는 열람실(ROOM_READS)에 묻는다.
_search_room = 'docs' not in globals()
_keys = globals().get('_len', {}) if _search_room else docs
import sys as _sys, os as _os, importlib as _il
for _d in __ROOMLIB__:
    if _d not in _sys.path: _sys.path.insert(0, _d)
import wiki_room_fuse as _fz
# 방은 한 번 불러온 모듈을 계속 쥔다 — 파일이 바뀌었으면 다시 불러온다(옛 모듈을 쥔 방은 벡터를 잃는다).
_mf = getattr(_fz, '__file__', None)  # 컨테이너 방은 모듈을 코드로 심어 파일이 없다
if _mf and getattr(_fz, '_room_mtime', None) != _os.path.getmtime(_mf):
    _il.reload(_fz); _fz._room_mtime = _os.path.getmtime(_mf)
if not at or age_h > MAX_AGE_H or meta.get('n') != len(_keys):
    print(f"STALE at={at} n={meta.get('n')} docs={len(_keys)}")
else:
    _tok = _fz.tok; _tok_re = _fz.TOK_RE
    stamp = (len(_keys), at, _fz.TOKV)
    if not _search_room and globals().get('_idx_stamp') != stamp:
        # 증분 색인: 저장마다 전량(11,600건 7초) 다시 만들지 않고 바뀐 문서만 빼고 더한다 — 방 동기화는 보통 1건.
        t0 = time.time()
        fresh, _nc, _nr = _fz.index_refresh(globals(), docs)
        _idx_stamp = stamp
        print(f"# 색인 {len(docs)}건 {'전량' if fresh else f'증분 +{_nc} -{_nr}'} {time.time()-t0:.2f}s")
    if globals().get('_prior_key') != (stamp, REFS_ALPHA):
        _prior = _fz.index_prior(_cites, _keys, REFS_ALPHA); _prior_key = (stamp, REFS_ALPHA)
    Nd = len(_keys); k1, b = 1.5, 0.75
    # 낱말 점수엔 부탁말('위키에서 찾아줘'·'알려줘')을 뺀다 — 제목에 같은 부탁말이 든 질의 페이지가 정답 정본을 밀어낸다.
    # 하이브리드: 질의를 적재와 같은 함수로 임베딩(로컬 우선)해 pgvector 상위를 받아 RRF 로 합친다(벡터는 PG에만 둔다).
    # 임베딩·DB 조회(입출력)는 스레드로 먼저 띄우고 그동안 BM25(계산)를 돈다 — 둘은 서로 독립이고 합치는 순서는 그대로(비동기 경로를 막지 않는다).
    # 임베딩·DB가 안 되면 BM25 만 쓰고 첫 줄에 사유를 남긴다(침묵 폴백 금지). 스레드 안 실패도 결과를 받을 때 그대로 드러난다.
    _vec_note = ''; _vec_fut = None; _vec_err = None; _card_box = {}
    try:
        import wiki_embed_client as _wc, psycopg2 as _pg, concurrent.futures as _cf
        _mf = getattr(_wc, '__file__', None)
        if _mf and getattr(_wc, '_room_mtime', None) != _os.path.getmtime(_mf):
            _il.reload(_wc); _wc._room_mtime = _os.path.getmtime(_mf)
        # 방이 ROOM_DOC_ROOT·ROOM_PG_DSN 변수를 두면 그것을 쓴다(컨테이너 안 방: 상대 경로 키·컨테이너 DB). 없으면 공통 접두 경로·__DSNFILE__의 접속 값.
        # 방 프로세스 환경의 LIBRARY_PG_DSN(컨테이너가 받은 접속 값)이 __DSNFILE__보다 먼저다.
        _root = globals().get('ROOM_DOC_ROOT')
        if _root is None:
            _root = _os.path.commonpath(list(_keys)[:2000]) if len(_keys) > 1 else _os.path.dirname(next(iter(_keys)))  # 위키 루트(어느 위키든 공통 접두 경로)
        _pfx = _root + '/' if _root else ''
        _dsn = globals().get('ROOM_PG_DSN') or _os.environ.get('LIBRARY_PG_DSN') or [l.split('=',1)[1].strip().strip('"\'') for l in open(__DSNFILE__)
                if l.startswith(('WIKI_EMBEDDINGS_DATABASE_URL=','DATABASE_URL='))][0].replace('postgresql+asyncpg://','postgresql://')
        # 방이 ROOM_EXTRA_ROOTS(위키 밖 서가 루트)를 두면 그 행도 함께 찾는다(서가 원문 캐시).
        _likes = [_pfx + '%'] + [r.rstrip('/') + '/%' for r in globals().get('ROOM_EXTRA_ROOTS', []) if r]
        _chunks_on = bool(globals().get('ROOM_CHUNKS'))
        # 방이 ROOM_CARD_PREFIXES(서지 레코드 경로 머리, 예 entities/스킬_)를 두면 서지 레코드만 따로 벡터로 찾아 목록 층을 낸다.
        _card_likes = [_pfx + c + '%' for c in globals().get('ROOM_CARD_PREFIXES', []) if c]
        def _vector_lookup():
            """질의 임베딩 → 문서 벡터 상위 20 · (켜면) 조각 순위 · 1단 후보 질의 페이지 2건. 방 변수는 서지 레코드 사본(_recs)만 쓴다."""
            global _recs, _recs_key
            _t = time.time(); _note = ''
            _qv, _model = _wc.embed(Q, 'query'); _card_box['qv'] = _qv
            with _pg.connect(_dsn, connect_timeout=3) as _c:
                with _c.cursor() as _cur:
                    # 한 표에 여러 위키가 있어 HNSW 가 거르기 전 40건만 보면 작은 위키는 0건 — 거른 뒤 LIMIT 을 채울 때까지 더 본다(pgvector 0.8).
                    _cur.execute("SET LOCAL hnsw.iterative_scan = strict_order")
                    # 문서 행·조각 행은 따로 부분 색인(wiki_emb_doc_hnsw·wiki_emb_chunk_hnsw)을 탄다.
                    # 탐색 폭 400: 전수 계산과 상위 겹침 0.99~1.00, 질의 약 6~8ms(기본 40은 0.78~0.93).
                    _cur.execute("SET LOCAL hnsw.ef_search = 400")
                    # 문서 벡터 행만(조각 행 '원문경로::c번호'는 뺀다).
                    _cur.execute("/* 벡터 상위 */ SELECT doc_path, embedding <=> %s::vector AS d FROM wiki_embeddings WHERE doc_path LIKE ANY(%s) AND doc_path NOT LIKE %s ORDER BY d LIMIT 20",
                                 (_fz.vec_literal(_qv), _likes, '%::c%'))
                    _rows = _cur.fetchall()
                    _v = [r[0] for r in _rows if r[0] in _keys]
                    # 최근접 거리 — 한 입구가 다른 도서관 결과를 낼지 가르는 근거(주석 줄이라 순위·채점과 무관).
                    if _rows: _note += f" · 최근접 {float(_rows[0][1]):.3f}"
                    if _card_likes:
                        # 서지 레코드: 켜거나 동기화된 뒤 처음 한 번만 DB 정본에서 검색실 메모리로 올리고, 질문마다 메모리에서 찾는다.
                        if globals().get('_recs_key') != (at, tuple(_card_likes)):
                            _cur.execute("/* 서지 레코드 올리기 */ SELECT doc_path, embedding::text FROM wiki_embeddings WHERE doc_path LIKE ANY(%s) AND doc_path NOT LIKE %s",
                                         (_card_likes, '%::c%'))
                            _recs = _fz.records_load(_cur.fetchall()); _recs_key = (at, tuple(_card_likes))
                            _note += f" · 서지 레코드 {len(_recs)}건 올림"
                        _card_box['cards'] = [c[0] for c in _fz.record_nearest(_qv, _recs, len(_recs)) if c[0] in _keys]
                    # 방이 ROOM_CHUNKS 를 켜면 벡터 목록을 조각 행(겹침 없는 1,000자) 순위로 바꾼다 — 문서 뒤쪽 사실까지 찾는다.
                    if _chunks_on:
                        _cur.execute("SELECT doc_path FROM wiki_embeddings WHERE doc_path LIKE ANY(%s) AND doc_path LIKE %s ORDER BY embedding <=> %s::vector LIMIT 100",
                                     (_likes, '%::c%', _fz.vec_literal(_qv)))
                        _cv = _fz.chunk_parents([r[0] for r in _cur.fetchall()], _keys, 20)
                        if _cv:
                            _v = _cv; _note += f" · 조각 벡터 {len(_cv)}문서"
                    # 1단(이중화) 후보: 질문과 가장 가까운 질의 페이지 두 건.
                    _cur.execute("SELECT doc_path, embedding <=> %s::vector AS d FROM wiki_embeddings WHERE doc_path LIKE %s AND doc_path NOT LIKE %s ORDER BY d LIMIT 4",
                                 (_fz.vec_literal(_qv), _pfx + 'queries/%', '%::c%'))
                    _q = [r for r in _cur.fetchall() if r[0] in _keys][:2]
            getattr(_c, 'close', lambda: None)()   # with 는 트랜잭션만 닫는다 — 접속은 여기서 닫아 방에 DB 연결이 남지 않게 한다(검색실은 색인만 든다)
            return _model, _v, _q, _note, time.time() - _t
        _vec_pool = _cf.ThreadPoolExecutor(max_workers=1)
        _vec_fut = _vec_pool.submit(_vector_lookup)
        _vec_pool.shutdown(wait=False)
    except Exception as _e:
        _vec_err = _e
    qt = _tok(re.sub(r'(위키에서|위키|찾아서|찾아줘|찾아|알려줘|해줘|있어|뭐야|뭐였어)', ' ', Q))
    scores = collections.defaultdict(float)
    if hasattr(_tf, 'bm25'):   # Rust index (runtime NativeIndex): same sums, same word order
        scores.update(_tf.bm25(list(set(qt)), Nd, k1, b, _avg))
    for w in (() if hasattr(_tf, 'bm25') else set(qt)):
        df = _df.get(w, 0)
        if not df: continue
        idf = math.log(1 + (Nd - df + 0.5)/(df + 0.5))
        for p, c in _tf.items():
            f = c.get(w)
            if not f: continue
            # 길이 감점에 상한(평균의 3배) — 본문 전체를 담은 서가 원문(수만 자)이 제목까지 다 맞아도 짧은 요약 쪽에 밀리지 않게.
            scores[p] += idf * f*(k1+1)/(f + k1*(1 - b + b*min(_len[p], 3*_avg)/_avg))
    # REFS 반영: 자라기 페이지가 '검색 결과'(REFS)로 인용한 횟수를 사전확률로 곱한다 — 실제로 근거가 된 문서가 위로 온다.
    det = [p for p, _ in sorted(((p, sc*_prior.get(p,1.0)) for p, sc in scores.items()), key=lambda x: -x[1])[:max(N, 20)]]
    try:
        if _vec_err is not None:
            raise _vec_err
        _model, vec, _qp, _vnote, _vsec = _vec_fut.result(timeout=30)
        _vec_note += _vnote; _anchors = []
        # 1단(이중화): 이미 물어본 질문이면 그 질의 페이지와 그 페이지가 인용한 문서를 맨 위에 고정하고 섞지 않는다.
        # 그 질의 페이지가 이번 BM25·벡터 상위에 없으면 고정하지 않는다(질의 페이지끼리만 가까운 엉뚱한 고정).
        if len(_qp) == 2:
            _qw = set(w for w in re.findall(r'[가-힣]{2,}|[a-z0-9]+', Q.lower()))
            _tw = set(w for w in re.findall(r'[가-힣]{2,}|[a-z0-9]+', _head.get(_qp[0][0], '').lower()))
            _idf = {w: math.log(1 + (Nd - _df.get(w, 0) + 0.5) / (_df.get(w, 0) + 0.5)) for w in (_qw | _tw)}
            _jac = _fz.weighted_jaccard(_qw, _tw, _idf); _gap = _qp[1][1] - _qp[0][1]
            if _fz.stage1_hit(_jac, _gap) and _fz.stage1_confirmed(_qp[0][0], det, vec):
                _bn2p = {p.rsplit('/', 1)[-1]: p for p in _keys}
                _anchors = [_qp[0][0]] + [_bn2p[b] for b in _refs.get(_qp[0][0], []) if b in _bn2p]
        # 가중치는 위키의 .wiki-pending/rrf_weights.json(베타 사후평균) — 판정 계수 단일 정의. 못 읽으면 0.5/0.5.
        _wd = _wv = 0.5
        try:
            import json as _js
            _st = _js.load(open(_root + '/.wiki-pending/rrf_weights.json', encoding='utf-8'))
            _wd = _st['det']['a'] / (_st['det']['a'] + _st['det']['b']); _wv = _st['vec']['a'] / (_st['vec']['a'] + _st['vec']['b'])
        except Exception as _we:
            _vec_note += f" (가중치 기본값: {type(_we).__name__})"
        # RRF k: 후보가 20건씩이라 k=60(천 건짜리 목록용)은 순위 차이를 지워 한쪽 1위가 밀린다 — 상수 하나(wiki_room_fuse.RRF_K).
        fused = _fz.anchor_first(_anchors, _fz.rrf_fuse(det, vec, _fz.RRF_K, _wd, _wv))
        if _anchors: _vec_note += f" · 1단 고정 {len(_anchors)}건(겹침 {_jac:.2f} 차이 {_gap:.3f})"
        # 베이즈 학습용 검색 기록 — 저장 글의 검색 결과와 대조해 위 가중치를 고친다. 모듈이 옛 판인 방이면 건너뛴다.
        if hasattr(_fz, 'retrieval_log'): _fz.retrieval_log(_root, Q, det, vec)
        _vec_note = f"# 하이브리드 {_model} 벡터 {len(vec)}건 {_vsec:.2f}s w {_wd:.2f}/{_wv:.2f}" + _vec_note
    except Exception as _e:
        fused = det; _vec_note = f"# 하이브리드 폴백(BM25만): {type(_e).__name__}: {str(_e)[:80]}"
    print(_vec_note)
    # 목록 층: 검색 순서 목록 → 백과사전 → 서가 — 서지 레코드를 결과 줄보다 먼저 낸다(주석 줄이라 정답지 채점과 무관).
    # 본문 검색처럼 벡터 순위와 낱말 순위를 섞는다. 거리로 거르지 않는다 — 어느 스킬을 쓸지는 읽는 쪽이 판단한다.
    if _card_box.get('cards'):
        _cr = {}; _cp = set(_card_box['cards'])
        for _lexc in (_card_box['cards'], [p for p, sc in sorted(scores.items(), key=lambda x: -x[1]) if sc > 0 and p in _cp]):
            for _i, _p in enumerate(_lexc): _cr[_p] = _cr.get(_p, 0) + 1 / (60 + _i + 1)
        for _i, _p in enumerate(sorted(_cr, key=lambda x: -_cr[x])[:3], 1):
            print(f"# 서지 레코드 {_i}. {_p[len(_pfx):] if _p.startswith(_pfx) else _p} — {_head.get(_p, '')}")
        del _cr, _cp, _lexc
    # 벡터가 꺼졌거나 실패하면(질의 벡터 없음) 서지 레코드를 낱말 점수로 고른다 — 서지 레코드 경로 머리 안의 BM25 상위 3.
    _cpre = [c for c in globals().get('ROOM_CARD_PREFIXES', []) if c]
    if _cpre and 'qv' not in _card_box:
        _cr = globals().get('ROOM_DOC_ROOT')
        if _cr is None:
            _cr = _os.path.commonpath(list(_keys)[:2000]) if len(_keys) > 1 else _os.path.dirname(next(iter(_keys)))
        _base = _cr.replace(_os.sep, '/').rstrip('/') + '/' if _cr else ''
        _cp = tuple(_base + c for c in _cpre)
        _lexc = [p for p, sc in sorted(scores.items(), key=lambda x: -x[1])
                 if sc > 0 and p.replace(_os.sep, '/').startswith(_cp)][:3]
        for _i, _p in enumerate(_lexc, 1):
            _rel = _p.replace(_os.sep, '/')[len(_base):]
            print(f"# 서지 레코드 {_i}. {_rel} — {_head.get(_p, '')} (낱말)")
    # 서지 레코드: 열람실 변수 ROOM_CATALOG(이름 → 백과사전 페이지, 또는 pages·items 서지 레코드)가 있으면 질문 속 이름의 백과사전을 고정하고 연결 항목을 그 아래에 낸다.
    _cat = globals().get('ROOM_CATALOG')
    if _cat:
        try:
            _pin, _link = _fz.catalog_hits(Q, _cat)
            _pin = [h for h in _pin if h in _keys]; _link = [h for h in _link if h in _keys]
            fused = _fz.layer_order(fused, _link, globals().get('ROOM_EXTRA_ROOTS', []), pinned=_pin)
            print(f"# 서지 레코드 · 백과사전 고정 {len(_pin)}건 · 연결 {len(_link)}건")
        except Exception as _le:
            print(f"# 층 순서 건너뜀: {type(_le).__name__}")
    # 열람실 ROOM_VERSION_GROUP이 있으면 그 묶음으로 한 칸. 없으면 파일 이름 판 접기.
    try:
        fused = _fz.apply_version_collapse(fused, globals().get('ROOM_VERSION_GROUP'))
    except Exception as _ce:
        print(f"# 판 접기 건너뜀: {type(_ce).__name__}")
    top = [(p, scores.get(p, 0.0)) for p in fused[:N]]
    words = [w for w in _tok_re.findall(Q.lower())]
    # 열람: 본문이 필요한 일(결과 줄·발췌)은 열람실에서 — 검색실이면 같은 서버의 열람실에 묻는다.
    def _read(fn, *args):
        if not _search_room:
            return getattr(_fz, fn)(docs, *args)
        return _fz.room_call(globals().get('ROOM_READS', ['wiki'])[0], _fz.READ_PRELUDE + f"print(_fz.{fn}(docs, *{args!r}), end='')")
    print(_read('read_lines', [p for p, _ in top], words, {p: _head.get(p, '') for p, _ in top}), end='')
    print(f"# 위키방 hit {len(top)} · sync {at} · {len(_keys)}건 · REFS인용문서 {len(_cites)} α={REFS_ALPHA}")
    # 전체 검색(_ENTRY)이면 오늘 들어온 글·회의실 변수·결정 모음을 함께 낸다 — 질의 벡터는 다시 만들지 않는다.
    if globals().get('_ENTRY'):
        # 방 변수 ROOM_SECTIONS(레포 사서가 등록부 선택으로 올림)가 있고 방 모듈이 그 선택을 받으면 그대로, 아니면 방 모듈 기본.
        # 컨테이너에서 호스트로 되돌린 실은 ROOM_SECTIONS를 지닌 채 선택을 모르는 옛 방 모듈을 쓸 수 있다.
        print(_fz.entry_sections(Q, _card_box.get('qv'), globals()['ROOM_SECTIONS'])
              if 'ROOM_SECTIONS' in globals() and _fz.entry_sections.__code__.co_argcount > 2
              else _fz.entry_sections(Q, _card_box.get('qv')), end='')
# 검색 뒤에는 이 질문의 계산 기록만 지운다 — 올려 둔 색인·서지 레코드는 남긴다. 시험·진단은 _SEARCH_KEEP 으로 남겨 들여다본다.
if not globals().get('_SEARCH_KEEP') and hasattr(_fz, 'forget_query'):  # 모듈이 옛 판인 방(배포 사이)이면 지우기만 건너뛴다
    _fz.forget_query(globals())
