"""Starting, checking, interrupting and stopping a room host process on Linux, macOS and Windows."""
import os
import signal
import subprocess
import sys

WINDOWS = os.name == "nt"
STILL_ACTIVE = 259                      # Windows exit code of a running process
QUERY_LIMITED_INFORMATION = 0x1000      # Windows process access right


def spawn_host(serve, argv):
    """Start the room host detached from the caller and return once it is on its own.

    Linux/macOS: double fork and setsid, then serve() in the grandchild; the first child is reaped here.
    Windows has no fork: a detached `python argv...` process starts the host. It outlives this command
    and is not waited for (Windows keeps no zombie)."""
    if WINDOWS:
        subprocess.Popen([sys.executable, *argv], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, close_fds=True,
                         creationflags=subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP)
        return
    child = os.fork()
    if child == 0:
        os.setsid()
        if os.fork() == 0:
            null = os.open(os.devnull, os.O_RDWR)  # the caller's pipes are let go so start returns at once
            for fd in (0, 1, 2):
                os.dup2(null, fd)
            serve()
        os._exit(0)
    os.waitpid(child, 0)


def is_alive(pid):
    if not WINDOWS:
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False
    # os.kill(pid, 0) would send CTRL_C_EVENT on Windows; ask the process handle instead.
    import ctypes
    kernel = ctypes.windll.kernel32
    kernel.OpenProcess.restype = ctypes.c_void_p
    handle = kernel.OpenProcess(QUERY_LIMITED_INFORMATION, False, pid)
    if not handle:
        return False
    try:
        code = ctypes.c_ulong()
        return bool(kernel.GetExitCodeProcess(ctypes.c_void_p(handle), ctypes.byref(code))) and code.value == STILL_ACTIVE
    finally:
        kernel.CloseHandle(ctypes.c_void_p(handle))


def terminate(pid):
    """Stop the host at once (SIGKILL; on Windows SIGTERM, which is TerminateProcess)."""
    os.kill(pid, signal.SIGTERM if WINDOWS else signal.SIGKILL)


def interrupt_main(thread_ident):
    """Raise SIGINT's handler in the host's main thread, where the running cell is."""
    if WINDOWS:
        import _thread
        _thread.interrupt_main(signal.SIGINT)
    else:
        signal.pthread_kill(thread_ident, signal.SIGINT)


def rss_mb(pid):
    """Resident memory in MB where /proc exists, else None."""
    try:
        with open(f"/proc/{pid}/status") as f:
            for ln in f:
                if ln.startswith("VmRSS:"):
                    return int(ln.split()[1]) // 1024
    except OSError:
        pass
    return None
