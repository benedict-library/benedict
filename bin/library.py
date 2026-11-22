#!/usr/bin/env python3
"""Standalone library: explicit local registry, lexical search and Markdown filing."""
import argparse
import json
import os
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import library_local


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="command", required=True)
    method = subs.add_parser("method", help="read how agents work in the library")
    method.add_argument("--lang", choices=("en", "ko"), default="en")
    incoming = subs.add_parser("import", help="preview an external Markdown document or skill folder, then import the approved selection")
    incoming.add_argument("--config", default="library.json")
    incoming.add_argument("--library", required=True, help="destination library key")
    incoming.add_argument("--approve", help="confirmation from the exact preview shown to the user")
    incoming.add_argument("kind", choices=("wiki", "skill"))
    incoming.add_argument("source", type=Path)
    source = subs.add_parser("web-source", help="archive a public source found with your agent's own web search")
    source.add_argument("--config", default="library.json")
    source.add_argument("--library", help="destination library; defaults to central")
    source.add_argument("question", help="public search terms")
    source.add_argument("url", help="public original URL returned by the agent")
    for name in ("init", "add", "index", "search", "read", "save"):
        sub = subs.add_parser(name)
        sub.add_argument("--config", default="library.json", help="local JSON registry; no deployment settings loaded")
        if name not in ("init", "add"):
            sub.add_argument("--library", help="registered library key; defaults to the sole central library")
        if name == "init":
            sub.add_argument("--demo", action="store_true", help="create fictional example documents")
            sub.add_argument("--key", default="main", help="registry key of the new library (default main)")
            sub.add_argument("--name", default="Example library", help="display name of the new library")
        elif name == "add":
            sub.add_argument("--key", required=True, help="key for a new project or specialist library")
            sub.add_argument("--name", help="display name (defaults to the key)")
        elif name == "search":
            sub.add_argument("--top", type=int, default=5)
            sub.add_argument("query")
        elif name == "read":
            sub.add_argument("path")
        elif name == "save":
            sub.add_argument("question")
            sub.add_argument("answer")
            sub.add_argument("--refs", default="")
    args = parser.parse_args(argv)
    try:
        if args.command == "method":
            guide = "method.ko.md" if args.lang == "ko" else "method.md"
            sys.stdout.write((Path(__file__).resolve().parents[1] / "docs" / guide).read_text(encoding="utf-8"))
            return 0
        if args.command == "import":
            from import_source import import_source
            result = import_source(args)
        elif args.command == "init":
            result = library_local.init(args.config, args.demo, args.key, args.name)
        elif args.command == "add":
            result = library_local.add(args.config, args.key, args.name)
        else:
            lib = library_local.Library(args.config)
            if args.command == "web-source":
                import web
                result = web.agent_source(lib, args.library, args.question, args.url)
            elif args.command == "index":
                result = lib.index(args.library)
            elif args.command == "search":
                result = lib.search(args.query, args.library, args.top)
            elif args.command == "read":
                result = lib.read(args.path, args.library)
            else:
                result = lib.save(args.question, args.answer, args.refs, args.library)
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except (OSError, ValueError, TypeError) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2


if __name__ == "__main__":
    if native := os.environ.get("BENEDICT_CLI"):
        os.execv(native, [native, *sys.argv[1:]])
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import common  # noqa: E402  (UTF-8 in and out, as every tool)
    common.utf8_stdio()
    sys.exit(main())
