#!/usr/bin/env python3
"""Publish committed local skills into an explicitly selected library catalog."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from skill_publish import publish


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", required=True, help="local committed source repository; no network access")
    parser.add_argument("--config", required=True, help="local library registry")
    parser.add_argument("--library", required=True, help="explicit destination; skill metadata does not route writes")
    parser.add_argument("--revision", default="HEAD", help="local Git commit/ref")
    parser.add_argument("--skill", action="append", help="publish only this skill; repeatable")
    parser.add_argument("--source-url", help="optional credential-free HTTPS Gitea-compatible repository URL, never fetched")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--plan", action="store_true",
                        help="return approved Markdown and expected previous digests without writing")
    args = parser.parse_args()
    try:
        result = publish(args.repo, args.config, args.library, args.revision, args.skill, args.source_url, args.dry_run or args.plan, plan=args.plan)
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False), file=sys.stderr)
        return 2
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import search  # noqa: E402  (records the use; see search.traced)
    # The native writer consumes complete Markdown bytes on this private channel.
    # Keep them inside the selected library rather than duplicating them in command traces.
    sys.exit(main() if "--plan" in sys.argv[1:] else search.traced("librarian", main))
