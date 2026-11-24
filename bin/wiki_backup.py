#!/usr/bin/env python3
"""Snapshot one local Markdown git repository to a verified bundle; restore to an empty path."""
import argparse
import hashlib
import io
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from library_local import Library


def git(root, *args, check=True):
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_TERMINAL_PROMPT="0", GIT_AUTHOR_NAME="Library snapshot",
               GIT_AUTHOR_EMAIL="snapshot@example.invalid", GIT_COMMITTER_NAME="Library snapshot",
               GIT_COMMITTER_EMAIL="snapshot@example.invalid")
    p = subprocess.run(["git", "-C", str(root), "-c", "core.hooksPath=/dev/null", "-c", "core.autocrlf=false", *args],
                       env=env, capture_output=True, text=True, encoding="utf-8", timeout=30)
    if check and p.returncode:
        raise ValueError("local git operation failed: " + p.stderr.strip()[:200])
    return p


def markdown(root):
    files = {}
    real_root = root.resolve()
    for path in sorted(root.rglob("*.md")):
        rel = path.relative_to(root)
        if any(p.startswith(".") for p in rel.parts):
            continue
        if path.is_symlink() or not path.resolve().is_relative_to(real_root):
            raise ValueError("symlink documents are not backed up")
        files[rel.as_posix()] = hashlib.sha256(path.read_bytes()).hexdigest()
    return files


def native_backup(*args):
    """Use the Rust command when installed; source-only hosts retain the original implementation."""
    binary = shutil.which("benedict")
    if not binary:
        return None
    result = subprocess.run([binary, "backup", *map(str, args)], capture_output=True, text=True, encoding="utf-8")
    if result.returncode:
        raise ValueError(result.stderr.strip() or "native backup failed")
    return json.loads(result.stdout)


def create_archive(path):
    """Exclusively create the archive with owner-only access from its first byte."""
    if os.name != "nt":
        return os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb")

    # Windows ignores POSIX mode bits. Provide a protected DACL at creation so
    # the parent directory's read permissions never reach the archive.
    import ctypes
    from ctypes import wintypes
    import msvcrt

    class SecurityAttributes(ctypes.Structure):
        _fields_ = [("length", wintypes.DWORD), ("descriptor", wintypes.LPVOID),
                    ("inherit_handle", wintypes.BOOL)]

    advapi = ctypes.WinDLL("advapi32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    convert = advapi.ConvertStringSecurityDescriptorToSecurityDescriptorW
    convert.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, ctypes.POINTER(wintypes.LPVOID),
                        ctypes.POINTER(wintypes.DWORD)]
    convert.restype = wintypes.BOOL
    create = kernel.CreateFileW
    create.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD,
                       ctypes.POINTER(SecurityAttributes), wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
    create.restype = wintypes.HANDLE
    kernel.LocalFree.argtypes = [wintypes.HLOCAL]
    kernel.LocalFree.restype = wintypes.HLOCAL
    kernel.CloseHandle.argtypes = [wintypes.HANDLE]
    descriptor = wintypes.LPVOID()
    # P protects the DACL; OW grants the file owner full access.
    if not convert("D:P(A;;FA;;;OW)", 1, ctypes.byref(descriptor), None):
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        attributes = SecurityAttributes(ctypes.sizeof(SecurityAttributes), descriptor, False)
        # GENERIC_WRITE, no sharing, CREATE_NEW, FILE_ATTRIBUTE_NORMAL.
        handle = create(str(path), 0x40000000, 0, ctypes.byref(attributes), 1, 0x80, None)
        if handle == wintypes.HANDLE(-1).value:
            raise ctypes.WinError(ctypes.get_last_error())
    finally:
        kernel.LocalFree(descriptor)
    try:
        fd = msvcrt.open_osfhandle(handle, os.O_WRONLY | os.O_BINARY)
    except OSError:
        kernel.CloseHandle(handle)
        raise
    return os.fdopen(fd, "wb")


def snapshot(config, key, out):
    native = native_backup("snapshot", "--config", config, "--out", out, *(("--library", key) if key else ()))
    if native is not None:
        return native
    _, _, root = Library(config).selected(key)
    out = Path(out).resolve()
    if out.exists() or out.is_relative_to(root):
        raise ValueError("backup target must be new and outside the wiki")
    if (root / ".git").is_symlink():
        raise ValueError("git directory must not be a symlink")
    if not (root / ".git").exists():
        git(root, "init", "-q")
    if Path(git(root, "rev-parse", "--show-toplevel").stdout.strip()).resolve() != root:
        raise ValueError("wiki must be its own git repository")
    files = markdown(root)
    if not files:
        raise ValueError("empty wiki has no snapshot content")
    tracked = git(root, "ls-files", "-z").stdout.split("\0")
    if any(p and p not in files for p in tracked):
        raise ValueError("repository includes non-Markdown or removed tracked files; resolve explicitly")
    for name in files:
        git(root, "add", "--", name)
    if git(root, "diff", "--cached", "--quiet", check=False).returncode:
        git(root, "commit", "-q", "-m", "Library Markdown snapshot")
    commit = git(root, "rev-parse", "HEAD").stdout.strip()
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="library-bundle-") as tmp:
        bundle = Path(tmp) / "wiki.bundle"
        git(root, "bundle", "create", str(bundle), "HEAD")
        git(root, "bundle", "verify", str(bundle))
        raw = bundle.read_bytes()
        manifest = {"version": 1, "commit": commit, "files": files,
                    "bundle_sha256": hashlib.sha256(raw).hexdigest()}
        with create_archive(out) as handle, tarfile.open(fileobj=handle, mode="w") as tar:
            for name, data in (("manifest.json", json.dumps(manifest).encode()), ("wiki.bundle", raw)):
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                entry.mode = 0o600
                tar.addfile(entry, io.BytesIO(data))
    return {"archive": str(out), "commit": commit, "files": len(files), "verified": True}


def restore(archive, target):
    native = native_backup("restore", "--archive", archive, "--target", target)
    if native is not None:
        return native
    target = Path(target).absolute()
    if target.exists() or target.is_symlink():
        raise ValueError("restore target must not exist")
    with tarfile.open(archive) as tar:
        members = tar.getmembers()
        if len(members) != 2 or {m.name for m in members} != {"manifest.json", "wiki.bundle"} or not all(m.isfile() for m in members):
            raise ValueError("invalid backup members")
        manifest = json.loads(tar.extractfile("manifest.json").read())
        data = tar.extractfile("wiki.bundle").read()
    if manifest.get("version") != 1 or hashlib.sha256(data).hexdigest() != manifest.get("bundle_sha256"):
        raise ValueError("backup hash mismatch")
    with tempfile.TemporaryDirectory(prefix="library-restore-") as tmp:
        bundle = Path(tmp) / "wiki.bundle"
        bundle.write_bytes(data)
        check = Path(tmp) / "verify"
        check.mkdir()
        git(check, "init", "-q")
        git(check, "bundle", "verify", str(bundle))
        staging = Path(tmp) / "wiki"
        git(check, "clone", "--quiet", "--no-checkout", str(bundle), str(staging))
        commit = git(staging, "rev-parse", "HEAD").stdout.strip()
        if commit != manifest.get("commit"):
            raise ValueError("snapshot commit mismatch")
        # Read blobs before checkout so a crafted bundle cannot create symlinks outside the wiki.
        entries = git(staging, "ls-tree", "-rz", "HEAD").stdout.split("\0")
        for entry in filter(None, entries):
            meta, name = entry.split("\t", 1)
            if not meta.startswith("100644 blob ") and not meta.startswith("100755 blob "):
                raise ValueError("snapshot must contain regular Markdown blobs")
            if not name.endswith(".md") or Path(name).is_absolute() or ".." in Path(name).parts:
                raise ValueError("snapshot contains invalid document path")
        git(staging, "checkout", "--quiet", "HEAD", "--", ".")
        if markdown(staging) != manifest.get("files"):
            raise ValueError("restored document hash mismatch")
        # Bundle origin is a disposable local file, never a deployment remote.
        git(staging, "remote", "remove", "origin")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(staging, target)
    return {"target": str(target), "commit": commit, "files": len(manifest["files"]), "verified": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    snap = sub.add_parser("snapshot")
    snap.add_argument("--config", required=True)
    snap.add_argument("--library")
    snap.add_argument("--out", required=True)
    back = sub.add_parser("restore")
    back.add_argument("--archive", required=True)
    back.add_argument("--target", required=True)
    args = parser.parse_args()
    try:
        result = snapshot(args.config, args.library, args.out) if args.command == "snapshot" else restore(args.archive, args.target)
        print(json.dumps(result))
        return 0
    except (OSError, ValueError, tarfile.TarError) as error:
        print(json.dumps({"error": str(error)}), file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import common  # noqa: E402  (records the use; see common.traced)
    sys.exit(common.traced("librarian", main))
