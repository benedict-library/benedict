"""Publish committed local skill originals with immutable Git provenance.

Ported from the committed-source/card workflow: no host-to-library routing, reviews
service, remote writes or implicit source URL. The destination is always explicit.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess
from urllib.parse import quote, urlsplit

from library_local import Library, atomic_text, confined

MARK = "<!-- library-skill-card -->"
PREFIX = "<!-- library-skill-source "
SHA = re.compile(r"[0-9a-f]{40,64}")
SECRET = re.compile(r"(?i)(?:password|passwd|secret|token|api[_-]?key)\s*[=:]\s*['\"]?([A-Za-z0-9_\-]{12,})|sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}")
ENV_NAME = re.compile(r"[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+")


def git(repo, *args, check=True):
    result = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, timeout=30)
    if check and result.returncode:
        raise ValueError("local Git operation failed: " + result.stderr.decode("utf-8", "replace")[:160])
    return result


def frontmatter(text):
    """Preserve original YAML; extract only scalar identity/description for the card."""
    text = text.replace("\r\n", "\n")
    if not text.startswith("---\n") or "\n---\n" not in text[4:]:
        raise ValueError("SKILL.md needs a closed YAML frontmatter block")
    front, body = text[4:].split("\n---\n", 1)
    fields = {}
    lines = front.splitlines()
    for i, line in enumerate(lines):
        match = re.match(r"^(name|description):\s*(.*)$", line)
        if not match:
            continue
        key, value = match.groups()
        if value in ("", ">", ">-", "|", "|-"):
            more = []
            for nxt in lines[i + 1:]:
                if not nxt.startswith((" ", "\t")):
                    break
                more.append(nxt.strip())
            value = " ".join(more)
        elif len(value) > 1 and value[0] == value[-1] and value[0] in "\"'":
            value = value[1:-1].replace('\\"', '"').replace("''", "'")
        fields[key] = value
    if not fields.get("name") or not fields.get("description") or not body.strip():
        raise ValueError("SKILL.md requires name, description and body")
    return fields


def leaks(text):
    return any(not m.group(1) or not ENV_NAME.fullmatch(m.group(1)) for m in SECRET.finditer(text))


def source_url(value):
    if value is None:
        return None
    url = urlsplit(value)
    if url.scheme != "https" or not url.hostname or url.username or url.password or url.query or url.fragment or any(c.isspace() for c in value):
        raise ValueError("source URL must be credential-free HTTPS without query or fragment")
    return value.rstrip("/")


def prior_revision(repo, old, commit, tree, source):
    if MARK not in old:
        raise ValueError("unmanaged destination preserved")
    found = re.search(re.escape(PREFIX) + r"(.*?) -->", old)
    if not found:
        raise ValueError("managed destination has no provenance")
    prior = json.loads(found.group(1))
    if not isinstance(prior, dict):
        raise ValueError("invalid published provenance")
    if not SHA.fullmatch(prior.get("commit", "")):
        raise ValueError("invalid published source commit")
    if prior.get("commit") != commit:
        previous = prior["commit"]
        if git(repo, "cat-file", "-e", previous + "^{commit}", check=False).returncode:
            raise ValueError("published commit unavailable locally; no automatic fetch")
        if git(repo, "merge-base", "--is-ancestor", commit, previous, check=False).returncode == 0:
            return "stale_release_skipped"
        if git(repo, "merge-base", "--is-ancestor", previous, commit, check=False).returncode:
            raise ValueError("divergent skill revision preserved; select a descendant release")
    if prior.get("tree_sha") == tree and prior.get("source_sha256") == source:
        return "unchanged"
    return "published"


def publish(repo, config, library, revision="HEAD", only=None, url=None, dry=False, plan=False):
    """A plan exposes policy-checked card bytes and the precondition for a separate atomic writer."""
    if plan and not dry:
        raise ValueError("a publication plan must be read-only")
    repo = Path(repo).resolve(strict=True)
    url = source_url(url)
    lib = Library(config)
    key, row, root = lib.selected(library)
    if not library:
        raise ValueError("an explicit destination --library is required")
    commit = git(repo, "rev-parse", "--verify", "--end-of-options", revision + "^{commit}").stdout.decode().strip()
    if git(repo, "status", "--porcelain", "--untracked-files=all", "--", "skills").stdout:
        raise ValueError("commit skill changes before publication; drafts are not published")
    paths = git(repo, "ls-tree", "-rz", "--name-only", commit, "--", "skills").stdout.split(b"\0")
    selected = []
    requested = set(only or [])
    for raw in paths:
        if not raw:
            continue
        path = raw.decode("utf-8")
        if not re.fullmatch(r"skills/[^/]+/SKILL\.md", path):
            continue
        name = path.split("/")[1]
        if requested and name not in requested:
            continue
        selected.append((name, path))
    missing = requested - {name for name, _ in selected}
    if missing:
        raise ValueError("unknown committed skills: " + ", ".join(sorted(missing)))
    if not selected:
        raise ValueError("no committed skills/*/SKILL.md documents")
    result = {"library": key, "commit": commit, "dry_run": dry,
              "published": [], "unchanged": [], "stale_release_skipped": []}
    planned = []
    for name, path in selected:
        mode = git(repo, "ls-tree", commit, "--", path).stdout.decode().split()[0]
        if mode != "100644" and mode != "100755":
            raise ValueError("SKILL.md must be a regular committed file: " + name)
        blob = git(repo, "show", commit + ":" + path).stdout
        text = blob.decode("utf-8")
        meta = frontmatter(text)
        if meta["name"] != name:
            raise ValueError("skill name differs from its committed directory: " + name)
        if leaks(text):
            raise ValueError("secret-like content rejected: " + name)
        if any(re.search(pattern, text, re.I) for pattern in row.get("save_block", {}).values()):
            raise ValueError("destination save rule rejected skill: " + name)
        tree = git(repo, "rev-parse", commit + ":skills/" + name).stdout.decode().strip()
        sha = hashlib.sha256(blob).hexdigest()
        relative = "entities/skill_" + name + ".md"
        target = confined(root, relative)
        old = target.read_text(encoding="utf-8") if target.exists() else None
        state = prior_revision(repo, old, commit, tree, sha) if old is not None else "published"
        result[state].append(name)
        if state != "published":
            continue
        provenance = {"commit": commit, "source_sha256": sha, "tree_sha": tree, "path": path}
        immutable = url + "/src/commit/" + commit + "/skills/" + quote(name) + "/" if url else None
        title = name + " — " + meta["description"]
        card = ("---\ntitle: " + json.dumps(title, ensure_ascii=False) + "\ntype: entity\ntags: [skill, catalog]\n---\n"
                + MARK + "\n" + PREFIX + json.dumps(provenance, sort_keys=True) + " -->\n\n"
                + "# " + name + "\n\n" + meta["description"] + "\n\n"
                + "- Commit: `" + commit + "`\n- SHA-256: `" + sha + "`\n- Skill tree: `" + tree + "`\n"
                + ("- Original: " + immutable + "SKILL.md\n- Companion files: " + immutable + "\n" if immutable else "- Original: `" + path + "` at the commit above (local Git source)\n")
                + "\n## Committed SKILL.md\n\n" + text.replace("\r\n", "\n"))
        planned.append((target, old, card))
    if plan:
        result["pages"] = [{"path": str(target.relative_to(root)),
                            "previous": hashlib.sha256(old.encode()).hexdigest() if old is not None else None,
                            "markdown": card} for target, old, card in planned]
    if not dry:
        for target, old, card in planned:
            current = target.read_text(encoding="utf-8") if target.exists() else None
            if current != old:
                raise ValueError("destination changed during publication; retry after inspection")
            atomic_text(target, card)
        if planned:
            lib.refresh(key)
    return result
