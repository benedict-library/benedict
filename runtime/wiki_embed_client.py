#!/usr/bin/env python3
"""Optional embedding adapter for hybrid search and embedding sync (ported from the existing deployment).

Loading documents and searching must use the same function so questions and documents share one vector space.
Settings are read from the environment on every call; nothing is configured by default.

  LIBRARY_EMBED_URL       local server with an OpenAI-style /v1/embeddings, e.g. http://127.0.0.1:11440 or
                          unix:///run/library-gate/embed.sock. Empty means embeddings are off (an error, not a guess).
  LIBRARY_EMBED_MODEL     model name written next to each stored vector (default "local")
  LIBRARY_EMBED_DIM       vector dimension (default 768)
  LIBRARY_EMBED_PROMPTS   "gemma" adds EmbeddingGemma task prompts; empty sends the text as is
  LIBRARY_EMBED_TIMEOUT   seconds per local request (default 20)
  LIBRARY_EMBED_FALLBACK  "gemini" falls back to the Gemini API with GEMINI_API_KEY when the local server fails;
                          the fallback is reported on stderr. Empty means no fallback.
Returns (vector, model name). Callers store the model name and re-embed rows whose model differs.
"""
from __future__ import annotations

import http.client
import json
import os
import re
import socket
import sys
import time
from urllib.error import HTTPError
from urllib.parse import urlparse
from urllib.request import Request, urlopen

GEMINI_MODEL = "gemini-embedding-2-preview"
MAX_CHARS = 8000
TRUNCATE_STEPS = (MAX_CHARS, 3000, 1500, 700)   # shorten and retry when the local server's context is exceeded


def _url():
    return os.environ.get("LIBRARY_EMBED_URL", "").strip().rstrip("/")


def _dim():
    return int(os.environ.get("LIBRARY_EMBED_DIM", "768"))


def model_name():
    return os.environ.get("LIBRARY_EMBED_MODEL", "").strip() or "local"


def format_text(text: str, kind: str, title: str = "") -> str:
    """kind: 'query' | 'doc'. With LIBRARY_EMBED_PROMPTS=gemma, adds the model card's task prompts."""
    text = (text or "")[:MAX_CHARS]
    if os.environ.get("LIBRARY_EMBED_PROMPTS", "") != "gemma":
        return text
    if kind == "query":
        return f"task: search result | query: {text}"
    return f"title: {title or 'none'} | text: {text}"


class _UnixConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self._path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self._path)


def _request(method: str, path: str, body, timeout: float) -> dict:
    url = _url()
    if not url:
        raise RuntimeError("embedding not configured (set LIBRARY_EMBED_URL)")
    data = json.dumps(body).encode("utf-8") if body is not None else None
    if url.startswith("unix://"):
        conn = _UnixConnection(urlparse(url).path, timeout)
        try:
            conn.request(method, path, body=data, headers={"Content-Type": "application/json"})
            resp = conn.getresponse()
            raw = resp.read()
            if resp.status >= 400:
                raise HTTPError(url + path, resp.status, resp.reason, resp.headers, None)
            return json.loads(raw.decode("utf-8"))
        finally:
            conn.close()
    req = Request(url + path, data=data, method=method, headers={"Content-Type": "application/json"})
    return json.loads(urlopen(req, timeout=timeout).read().decode("utf-8"))


def _timeout():
    return float(os.environ.get("LIBRARY_EMBED_TIMEOUT", "20"))


def embed_local(text: str, kind: str, title: str = "") -> list[float]:
    """One local embedding. A 4xx/5xx caused by input length is retried with a shorter beginning."""
    last = None
    for i, n in enumerate(TRUNCATE_STEPS):
        try:
            d = _request("POST", "/v1/embeddings", {"input": format_text((text or "")[:n], kind, title)}, _timeout())
            vec = d["data"][0]["embedding"]
            if len(vec) != _dim():
                raise ValueError(f"local dim {len(vec)} != {_dim()}")
            return vec
        except HTTPError as e:
            last = e
            if e.code not in (400, 413, 422, 500) or i == len(TRUNCATE_STEPS) - 1:
                raise
    raise last


def embed_local_batch(texts: list[str], kind: str, title: str = "") -> list[list[float]]:
    """Several local embeddings in one request; on failure, one at a time (with the shortening retry)."""
    inp = [format_text(t, kind, title) for t in texts]
    try:
        d = _request("POST", "/v1/embeddings", {"input": inp}, _timeout() * max(1, len(inp) // 4))
        vecs = [x["embedding"] for x in d["data"]]
        if len(vecs) == len(inp) and all(len(v) == _dim() for v in vecs):
            return vecs
    except HTTPError:
        pass
    return [embed_local(t, kind, title) for t in texts]


def embed_gemini(text: str) -> list[float]:
    key = os.environ.get("GEMINI_API_KEY", "").strip()
    if not key:
        raise RuntimeError("GEMINI_API_KEY not set")
    url = f"https://generativelanguage.googleapis.com/v1beta/models/{GEMINI_MODEL}:embedContent?key={key}"
    body = {"model": f"models/{GEMINI_MODEL}", "content": {"parts": [{"text": (text or "")[:MAX_CHARS]}]},
            "outputDimensionality": _dim()}
    last = None
    for attempt in range(3):
        try:
            req = Request(url, data=json.dumps(body).encode("utf-8"), method="POST",
                          headers={"Content-Type": "application/json"})
            d = json.loads(urlopen(req, timeout=45).read().decode("utf-8"))
            emb = d.get("embedding")
            vals = emb.get("values") if isinstance(emb, dict) else emb
            if not vals or len(vals) != _dim():
                raise ValueError("bad embedding dim")
            return vals
        except Exception as e:  # 429 and transient errors: back off
            last = e
            if attempt < 2:
                time.sleep(5 * (attempt + 1))
    raise last


def embed(text: str, kind: str = "doc", title: str = "", allow_fallback: bool = True) -> tuple[list[float], str]:
    """(vector, model name). Falls back to Gemini only when LIBRARY_EMBED_FALLBACK=gemini, and says so on stderr."""
    try:
        return embed_local(text, kind, title), model_name()
    except Exception as e:
        if not allow_fallback or os.environ.get("LIBRARY_EMBED_FALLBACK", "") != "gemini" or not _url():
            raise
        sys.stderr.write(f"[embedding] local failed ({type(e).__name__}) -> Gemini fallback\n")
        return embed_gemini(text), GEMINI_MODEL


def local_alive(timeout: float = 3) -> bool:
    try:
        return _request("GET", "/health", None, timeout).get("status") == "ok"
    except Exception:
        return False


# Chunks: 1,000 characters without overlap, up to 30 per document, stored as 'path::cN' rows in the same table.
CHUNK_CHARS = 1000
CHUNK_MAX = 30
CHUNK_SEP = "::c"
_FM_RE = re.compile(r"\A---\n.*?\n---[ \t]*\n", re.S)


def chunk_texts(text: str, size: int = CHUNK_CHARS, cap: int = CHUNK_MAX) -> list[str]:
    """Drop the front matter and cut the body into size-character pieces (no empty pieces, at most cap)."""
    body = _FM_RE.sub("", text or "", count=1)
    out = [body[i:i + size] for i in range(0, len(body), size)]
    return [c for c in out if c.strip()][:cap]


def chunk_key(path: str, i: int) -> str:
    return f"{path}{CHUNK_SEP}{i}"


def chunk_parent(key: str):
    """The original path of a chunk row, or None."""
    if CHUNK_SEP not in key:
        return None
    head, tail = key.rsplit(CHUNK_SEP, 1)
    return head if tail.isdigit() else None


if __name__ == "__main__":
    kind = sys.argv[1] if len(sys.argv) > 1 else "query"
    vec, model = embed(" ".join(sys.argv[2:]) or "test", kind)
    print(json.dumps({"model": model, "dim": len(vec), "head": [round(x, 4) for x in vec[:4]]}))
