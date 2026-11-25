#!/usr/bin/env python3
"""Publish chosen TCP services as Unix sockets for a library container that has no network.

  gate.py --route /gate/embed.sock=127.0.0.1:11440 --route /gate/.s.PGSQL.5432=127.0.0.1:5432

Run it in a separate container (or process) that can reach those services, and share the socket folder with the
library container. The library container then reaches exactly these services and nothing else.
Standard library only.
"""
import argparse
import asyncio
import os
import sys


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        writer.close()


async def serve(path, host, port):
    async def handle(reader, writer):
        try:
            target_reader, target_writer = await asyncio.open_connection(host, port)
        except OSError:
            writer.close()
            return
        await asyncio.gather(pipe(reader, target_writer), pipe(target_reader, writer))

    if os.path.exists(path):
        os.unlink(path)
    # Create the socket with its final mode: a chmod after bind leaves a moment when it is wider.
    mask = os.umask(0o117)
    try:
        server = await asyncio.start_unix_server(handle, path)
    finally:
        os.umask(mask)
    os.chmod(path, 0o660)
    return server


def parse_route(text):
    path, _, target = text.partition("=")
    host, _, port = target.rpartition(":")
    if not path.startswith("/") or not host or not port.isdigit():
        raise argparse.ArgumentTypeError(f"route must be /socket/path=host:port, got {text!r}")
    return path, host, int(port)


async def main(routes):
    servers = [await serve(*r) for r in routes]
    print("gate ready: " + ", ".join(f"{p} -> {h}:{port}" for p, h, port in routes), flush=True)
    await asyncio.gather(*(s.serve_forever() for s in servers))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--route", type=parse_route, action="append", required=True)
    try:
        asyncio.run(main(parser.parse_args().route))
    except KeyboardInterrupt:
        sys.exit(0)
