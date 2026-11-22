# 실 읽기 코드 — bin/read.py 가 채워 열람실(메모리)에 exec 로 보낸다. 원문 사본을 열람실 변수에 두고 화면엔 필요한 대목만 낸다.
# 채우는 자리: __PATHS__(원본 경로 목록) __VAR__(열람실 변수 이름) __WORDS__(질문 낱말) __LINES__((시작, 끝) 또는 None) __HEAD__(앞 줄 수)
#            __LOCAL__({경로: 본문} — 보내는 기기에만 있는 파일, 본문을 실어 왔다)
# 명시적으로 첨부한 본문 또는 선택한 도서관의 원본 파일만 읽는다. docs 의 색인용 사본은 원본을 대신하지 않는다.
import datetime as _zzdt
import hashlib as _zzhash
import re as _zzre
from pathlib import Path as _zzPath
_zzvn, _zzrange = __VAR__, __LINES__
_zzv = globals().setdefault(_zzvn, {})
_zzdocs = globals().get('docs') or {}
_zzlocal = __LOCAL__
_zzroot = __ROOT__
for _zzp in __PATHS__:
    try:
        if _zzp in _zzlocal:
            _zzbytes = _zzlocal[_zzp].encode('utf-8')
        else:
            if not _zzroot:
                raise PermissionError('no library root for reading an original')
            _zzfile = _zzPath(_zzp).resolve()
            if not _zzfile.is_relative_to(_zzPath(_zzroot).resolve()):
                raise PermissionError('path outside selected library')
            _zzbytes = _zzfile.read_bytes()
        _zzt = _zzbytes.decode('utf-8').replace('\r\n', '\n').replace('\r', '\n')
        if (any(_zzre.search(rule, _zzt) for rule in globals().get('ROOM_INDEX_BLOCK_RULES', {}).values())
                or '> 개인정보가 들어 있어 본문은 색인하지 않았습니다.' in _zzdocs.get(_zzp, '')):
            raise PermissionError('index policy excludes this source from room memory; read the original with the local CLI')

    except (OSError, UnicodeError) as _zze:
        print(f'## {_zzp}\n(못 읽음: {type(_zze).__name__}: {_zze})')
        continue
    _zzv[_zzp] = {'text': _zzt, 'sha256': _zzhash.sha256(_zzbytes).hexdigest(), 'read_at': _zzdt.datetime.now().isoformat(timespec='seconds')}
    _zzl = _zzt.split('\n')
    print(f'## {_zzp} ({len(_zzl)}줄) → 열람실 변수 {_zzvn}')
    if _zzrange:
        _zzshow = list(range(max(1, _zzrange[0]) - 1, min(len(_zzl), _zzrange[1])))
    elif __WORDS__:
        _zzk, _zzh = set(), set()
        for _zzi, _zzx in enumerate(_zzl):
            if any(_zzw in _zzx.lower() for _zzw in __WORDS__):
                _zzh.add(_zzi)
                _zzk.update(range(max(0, _zzi - 1), min(len(_zzl), _zzi + 2)))
        _zzshow = sorted(_zzk)
    else:
        _zzshow = sorted({_zzi for _zzi, _zzx in enumerate(_zzl) if _zzx.startswith(('#', 'title:'))} | set(range(min(len(_zzl), __HEAD__))))
    if not _zzshow:
        print('(질문 낱말이 든 줄 없음 — --lines 로 범위를 꺼낸다)')
    # 한 문서에서 화면에 내는 줄은 80줄까지 — 나머지는 열람실 변수에 있으니 --lines 로 꺼낸다.
    # 고른 범위(--lines)와 낱말 든 줄은 전문을 낸다. 곁 줄·머리 보기의 400자 넘는 줄만 줄이고, 줄인 것과 전문 꺼내는 법을 그 줄에 밝힌다.
    _zzfull = set(_zzshow) if _zzrange else _zzh if __WORDS__ else set()
    _zzprev = None
    for _zzi in _zzshow[:80]:
        if _zzprev is not None and _zzi != _zzprev + 1:
            print('…')
        _zzx = _zzl[_zzi]
        if _zzi in _zzfull or len(_zzx) <= 400:
            print(f'{_zzi + 1}: {_zzx}')
        else:
            print(f'{_zzi + 1}: {_zzx[:400]} …(+{len(_zzx) - 400}자 — 전문은 --lines {_zzi + 1}-{_zzi + 1})')
        _zzprev = _zzi
    if len(_zzshow) > 80:
        print(f'… 나머지 {len(_zzshow) - 80}줄은 --lines 로')
for _zzn in ('_zzdt', '_zzhash', '_zzre', '_zzPath', '_zzroot', '_zzfile', '_zzbytes', '_zzvn', '_zzrange', '_zzv', '_zzdocs', '_zzlocal', '_zzp', '_zzt', '_zze', '_zzl', '_zzshow', '_zzk', '_zzh', '_zzfull', '_zzi', '_zzx', '_zzw', '_zzprev'):
    globals().pop(_zzn, None)
del _zzn
