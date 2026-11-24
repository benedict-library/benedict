#!/usr/bin/env python3
"""Measure local retrieval rank and optional offline answer-key coverage.

Ported from library_eval.py hit_rank/summary and library_answer_eval.py grade.
No answer generation, credentials or network calls. Key coverage is not semantic correctness.
"""
import argparse
import datetime
import json
from pathlib import Path, PurePosixPath
import re
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from library_local import Library, atomic_text


def jsonl(path):
    rows = []
    for number, line in enumerate(Path(path).read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError as error:
            raise ValueError(f"{path}: line {number}: invalid JSON") from error
        if not isinstance(row, dict):
            raise ValueError(f"{path}: line {number}: expected object")
        rows.append(row)
    if not rows:
        raise ValueError(f"{path}: empty evaluation data")
    return rows


def documents(answer):
    values = [answer] if isinstance(answer, str) else answer
    if not isinstance(values, list) or not values or any(not isinstance(x, str) or not x.strip() for x in values):
        raise ValueError("answer_doc must be a document string or nonempty string list")
    for value in values:
        p = PurePosixPath(value)
        if p.is_absolute() or ".." in p.parts or p.suffix != ".md" or "\\" in value:
            raise ValueError("answer_doc must contain relative Markdown paths or basenames")
    return values


def hit_rank(paths, answers, corpus):
    wanted = set()
    for answer in documents(answers):
        if "/" in answer:
            wanted.add(PurePosixPath(answer).as_posix())
        else:
            candidates = [p for p in corpus if PurePosixPath(p).name == answer]
            if len(candidates) > 1:
                raise ValueError(f"ambiguous answer_doc basename: {answer}; use a relative path")
            wanted.update(candidates or [answer])
    return next((i for i, path in enumerate(paths, 1) if path in wanted), None)


def grade(answer, keys):
    if not isinstance(keys, list) or not keys or any(not isinstance(key, str) or any(not alt.strip() for alt in key.split("|")) for key in keys):
        raise ValueError("offline grading requires nonempty keys with nonempty alternatives")
    norm = lambda text: re.sub(r"\s+", "", text).casefold()
    text = norm(answer)
    missing = [key for key in keys if not any(norm(alt) in text for alt in key.split("|"))]
    return not missing, missing


def evaluate(config, golden, key=None, k=8, answers=None):
    if not 1 <= k <= 100:
        raise ValueError("k must be between 1 and 100")
    questions = jsonl(golden)
    seen = set()
    for row in questions:
        q = row.get("q")
        if not isinstance(q, str) or not q.strip() or q in seen:
            raise ValueError("golden requires unique nonempty q strings")
        seen.add(q)
        documents(row.get("answer_doc"))
    supplied = None
    if answers is not None:
        supplied = {}
        for row in jsonl(answers):
            q, answer = row.get("q"), row.get("answer")
            if not isinstance(q, str) or q in supplied or q not in seen or not isinstance(answer, str):
                raise ValueError("answers require unique golden q and string answer")
            supplied[q] = answer
        if set(supplied) != seen:
            raise ValueError("answers must cover every golden question exactly once")
    lib = Library(config)
    library, corpus = lib.documents(key)
    rows = []
    stamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
    for g in questions:
        started = time.perf_counter()
        found = lib.search(g["q"], key, 100)
        paths = [hit["path"] for hit in found["hits"]]
        rank = hit_rank(paths, g["answer_doc"], corpus)
        # Reading the returned documents verifies the retrieval-to-reader path as well.
        for path in paths[:k]:
            lib.read(path, key)
        row = {"ts": stamp, "q": g["q"], "answer_doc": g["answer_doc"], "rank": rank,
               "secs": round(time.perf_counter() - started, 6), "rc": 0, "top": paths[:k]}
        row.update({name: g[name] for name in ("src", "made_by") if name in g})
        if supplied is not None:
            covered, missing = grade(supplied[g["q"]], g.get("keys"))
            row.update({"key_coverage_pass": covered, "missing_keys": missing,
                        "answer": supplied[g["q"]], "keys": g["keys"]})
        rows.append(row)
    secs = sum(row["secs"] for row in rows)
    hit = sum(row["rank"] is not None and row["rank"] <= k for row in rows)
    summary = {"n": len(rows), "hit": hit, "k": k, "avg_secs": secs / len(rows),
               "quality_per_sec": hit / secs if secs else None,
               "answer_metric": "key-coverage-not-semantic-correctness" if supplied is not None else "not-evaluated"}
    if supplied is not None:
        summary["key_coverage_pass"] = sum(row["key_coverage_pass"] for row in rows)
    passed = hit == len(rows) and (supplied is None or summary["key_coverage_pass"] == len(rows))
    return {"library": library, "engine": "lexical-bm25", "golden": Path(golden).name,
            "passed": passed, "rows": rows, "summary": summary}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default="library.json")
    parser.add_argument("--golden", required=True)
    parser.add_argument("--library")
    parser.add_argument("--k", type=int, default=8)
    parser.add_argument("--answers", help="offline JSONL objects {q, answer}; no answer generator is called")
    parser.add_argument("--out", help="write complete JSON report; replaces only the requested report path")
    args = parser.parse_args(argv)
    try:
        if args.out and Path(args.out).resolve() in {Path(p).resolve() for p in (args.config, args.golden, args.answers) if p}:
            raise ValueError("out must not overwrite an input")
        report = evaluate(args.config, args.golden, args.library, args.k, args.answers)
        text = json.dumps(report, ensure_ascii=False, indent=2)
        if args.out:
            atomic_text(Path(args.out), text + "\n")
        print(text)
        return 0 if report["passed"] else 1
    except (OSError, ValueError, TypeError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import common  # noqa: E402  (records the use; see common.traced)
    sys.exit(common.traced("librarian", main))
