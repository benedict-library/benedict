"""Import an explicitly selected source without modifying it or replacing an existing document."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
from tempfile import TemporaryDirectory

from library_local import Library, confined
from save_query import write_lock
from skill_publish import frontmatter, git, leaks, publish


def inventory(kind, source):
    if kind == "wiki":
        if not source.is_file() or source.suffix != ".md":
            raise ValueError("select one Markdown wiki file")
        return [source]
    if not source.is_dir() or not (source / "SKILL.md").is_file():
        raise ValueError("select a skill directory containing SKILL.md")
    files = []

    def raise_error(error):
        raise error

    for parent, dirs, names in os.walk(source, onerror=raise_error):
        dirs[:] = [name for name in dirs if not name.startswith(".")]
        for name in dirs + [n for n in names if not n.startswith(".")]:
            path = Path(parent, name)
            if path.is_symlink():
                raise ValueError(f"nested symlink requires a separate selection: {path}")
            if name not in dirs:
                if not stat.S_ISREG(path.stat().st_mode):
                    raise ValueError(f"not a regular file: {path}")
                files.append(path)
    return sorted(files)


def import_source(args):
    lib = Library(args.config)
    key, row, root = lib.selected(args.library)
    source = args.source.expanduser().resolve(strict=True)
    files = inventory(args.kind, source)
    base = source if args.kind == "skill" else source.parent
    names = [str(p.relative_to(base)) for p in files]
    plan = dict(kind=args.kind, source=str(source), library=key, wiki=str(root),
                registry=str(lib.config), files=names)
    versions = [(s.st_dev, s.st_ino, s.st_size, s.st_mtime_ns) for s in (p.stat() for p in files)]
    token = hashlib.sha256(json.dumps([plan, versions], sort_keys=True).encode()).hexdigest()
    if args.approve is None:
        return dict(plan, confirmation=token, status="awaiting approval")
    if args.approve != token:
        raise ValueError("source or destination differs from the approved preview; preview again")

    # Stage approved bytes before making anything discoverable. Reject unreadable sources as a unit.
    with TemporaryDirectory() as tmp:
        snapshot = Path(tmp) / "snapshot"
        snapshot.mkdir()
        for path, name, version in zip(files, names, versions):
            if path.resolve(strict=True) != path:
                raise ValueError("source path changed; preview again")
            with path.open("rb") as opened:
                current = os.fstat(opened.fileno())
                if (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns) != version:
                    raise ValueError("source changed; preview again")
                data = opened.read()
                after = os.fstat(opened.fileno())
                if after.st_size != current.st_size or after.st_mtime_ns != current.st_mtime_ns:
                    raise ValueError("source changed while reading; preview again")
            out = snapshot / name
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(data)
            if args.kind == "skill":
                out.chmod(0o755 if current.st_mode & 0o111 else 0o644)
            if out.read_bytes() != data:
                raise OSError("staged copy differs from source")
        with write_lock(lib.config.parent / ".library-state" / "import.lock"):
            if args.kind == "wiki":
                return import_wiki(lib, key, row, root, source.name, snapshot / names[0])
            return import_skill(lib, key, row, snapshot)


def import_wiki(lib, key, row, root, filename, staged):
    relative = "imports/" + filename
    dest = confined(root, relative)
    data = staged.read_bytes()
    text = data.decode("utf-8")
    if any(re.search(pattern, text, re.I) for pattern in row.get("save_block", {}).values()):
        raise ValueError("destination save rule rejected document")
    if any(part.startswith(".") for part in Path(relative).parts):
        raise ValueError("hidden wiki documents cannot be indexed")
    if not any(Path(relative).is_relative_to(folder) for folder in row.get("dirs") or ["."]):
        raise ValueError("imports directory is outside this library’s indexed directories")
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists():
        if dest.read_bytes() != data:
            raise ValueError("existing document differs; preserved")
    else:
        with TemporaryDirectory(dir=dest.parent) as staging:
            ready = Path(staging) / "ready"
            ready.write_bytes(data)
            ready.chmod(0o600)
            os.link(ready, dest)
    lib.refresh(key, relative)
    return dict(status="imported", library=key, path=relative)


def import_skill(lib, key, row, snapshot):
    text = (snapshot / "SKILL.md").read_bytes().decode("utf-8")
    name = frontmatter(text)["name"]
    if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        raise ValueError("invalid skill name")
    if leaks(text) or any(re.search(p, text, re.I) for p in row.get("save_block", {}).values()):
        raise ValueError("destination save rule rejected skill")
    # Preserve the entire selected tree beside the registry so a successor can open companion files.
    if Path(key).name != key or key in (".", ".."):
        raise ValueError("invalid destination key for an import shelf")
    shelves = confined(lib.config.parent, Path(".library-state/import-shelves") / key)
    shelves.mkdir(parents=True, exist_ok=True)
    repo = shelves / name
    if repo.is_symlink():
        raise ValueError("import shelf must not be a symlink")
    with TemporaryDirectory(dir=shelves) as tmp:
        candidate = Path(tmp) / "repo"
        tree = candidate / "skills" / name
        tree.parent.mkdir(parents=True)
        shutil.copytree(snapshot, tree)
        git(candidate, "init", "-q")
        git(candidate, "config", "core.autocrlf", "false")
        git(candidate, "add", "-f", "--", "skills")
        git(candidate, "-c", "user.name=Library import", "-c", "user.email=library@localhost",
            "-c", "commit.gpgsign=false", "commit", "-qm", "Import approved skill")
        if repo.exists():
            if git(repo, "rev-parse", "HEAD:skills").stdout != git(candidate, "rev-parse", "HEAD:skills").stdout:
                raise ValueError("an imported skill already has different contents; existing source preserved")
        else:
            publish(candidate, lib.config, key, only=[name], dry=True)
            candidate.rename(repo)
    result = publish(repo, lib.config, key, only=[name])
    if not result["published"]:
        lib.refresh(key)
    return dict(result, status="imported", shelf=str(repo))
