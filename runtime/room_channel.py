"""The local channel between a room host and its callers.

Where the system has Unix sockets (Linux, macOS) the channel file ROOM.sock is the socket itself (0600).
Elsewhere (Windows) the host listens on 127.0.0.1 at a free port and ROOM.sock is a 0600 file holding
"PORT TOKEN"; a caller sends the token as its first line and the host closes a connection whose token differs.
"""
from contextlib import contextmanager
import hmac
import os
import secrets
import socket
import tempfile

TCP = not hasattr(socket, "AF_UNIX")


@contextmanager
def unix_address(path):
    # BSD sockaddr_un has 104 bytes including the trailing NUL. The kernel
    # limits the supplied pathname, not the location of the socket inode.
    if len(os.fsencode(path)) < 104:
        yield path
        return
    directory, name = os.path.split(os.path.abspath(path))
    with tempfile.TemporaryDirectory(dir="/tmp", prefix="room-") as private:
        alias = os.path.join(private, "d")
        os.symlink(directory, alias)
        yield os.path.join(alias, name)


class Listener:
    def __init__(self, path, tcp=TCP):
        self.path, self.token = path, None
        if os.path.exists(path):
            os.unlink(path)
        if not tcp:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            with unix_address(path) as address:
                self.sock.bind(address)
            os.chmod(path, 0o600)
        else:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.sock.bind(("127.0.0.1", 0))
            self.token = secrets.token_hex(32)
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(f"{self.sock.getsockname()[1]} {self.token}\n")
        self.sock.listen(16)

    def accept(self):
        """(connection, its read stream) for the next caller, or None when its token was wrong."""
        conn, _ = self.sock.accept()
        rfile = conn.makefile("rb")
        if self.token is not None and not hmac.compare_digest(rfile.readline().strip(), self.token.encode()):
            rfile.close()
            conn.close()
            return None
        return conn, rfile

    def close(self):
        self.sock.close()
        try:
            os.unlink(self.path)
        except OSError:
            pass


def connect(path, timeout, tcp=TCP):
    """A connected socket to the room at path; FileNotFoundError or ConnectionRefusedError when it is closed."""
    if not tcp:
        c = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        c.settimeout(timeout)
        try:
            with unix_address(path) as address:
                c.connect(address)
        except OSError:
            c.close()
            raise
        return c
    with open(path) as f:
        port, token = f.read().split()
    c = socket.create_connection(("127.0.0.1", int(port)), timeout=timeout)
    c.sendall(token.encode() + b"\n")
    return c
