#!/usr/bin/env python3
"""A logging HTTP proxy for the network audit. See docs/unaligned/NETWORK.md.

    script/unaligned/netlog-proxy.py <port> <logfile>

Listens on 127.0.0.1 only. Port 0 picks a free port. The port is printed on standard output
(one line), then the proxy serves until it is terminated. One line is appended to <logfile> for
each connection a client asks for:

    2026-11-12T08:50:01.123Z CONNECT api.example.com:443        an HTTPS (or any TCP) tunnel
    2026-11-12T08:50:01.456Z GET localhost:11434 /api/tags      a plain HTTP request
    # 2026-11-12T08:50:01.789Z FAILED localhost:11434 Connection refused
    # 2026-11-12T08:50:09.000Z CLOSED api.example.com:443 sent=1840 received=5122 last=2026-11-12T08:50:02.250Z

Lines starting with # are remarks about a connection logged above: FAILED when the server could
not be reached; CLOSED when the connection ended (or the proxy was stopped), with the bytes that
went to the server and came back, and the time of the last one.

CONNECT tunnels carry the bytes untouched: nothing is decrypted, so for HTTPS only the host, the
port and the volume are known. Plain HTTP requests are forwarded with their headers and body as
they came; only the method, the host and the path are logged (never the query string, the
headers or the body). Python 3 standard library only.
"""
import select
import signal
import socket
import socketserver
import sys
import threading
import time
from datetime import datetime, timezone
from urllib.parse import urlsplit

HEAD_LIMIT = 64 * 1024
CONNECT_TIMEOUT = 15
HOP_HEADERS = (b"proxy-connection", b"proxy-authorization", b"connection", b"keep-alive")

log_lock = threading.RLock()
log_file = None
open_connections = set()


def stamp(seconds):
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def log(line, comment=False):
    with log_lock:
        log_file.write(f"{'# ' if comment else ''}{stamp(time.time())} {line}\n")
        log_file.flush()


class Connection:
    """Byte counts of one connection to a server, for its CLOSED line."""

    def __init__(self, endpoint):
        self.endpoint = endpoint
        self.sent = self.received = 0
        self.last = time.time()
        self.closed = False
        with log_lock:
            open_connections.add(self)

    def close(self):
        with log_lock:
            if not self.closed:
                self.closed = True
                open_connections.discard(self)
                log(f"CLOSED {self.endpoint} sent={self.sent} received={self.received} last={stamp(self.last)}", comment=True)


def split_host(text, default_port):
    """'host', 'host:port' or '[v6]:port' -> (host, port)."""
    if text.startswith("["):
        host, _, rest = text[1:].partition("]")
        port = rest.lstrip(":")
    else:
        host, _, port = text.partition(":")
    return host.lower().rstrip("."), int(port) if port else default_port


def name(host, port):
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def relay(client, upstream, close_on_client_eof, connection):
    """Copies bytes both ways until the connection is over."""
    open_sockets = {client: upstream, upstream: client}
    while open_sockets:
        readable, _, _ = select.select(list(open_sockets), [], [])
        for source in readable:
            target = open_sockets.get(source)
            if target is None:
                continue
            try:
                data = source.recv(65536)
            except OSError:
                return
            if data:
                if source is client:
                    connection.sent += len(data)
                else:
                    connection.received += len(data)
                connection.last = time.time()
                try:
                    target.sendall(data)
                except OSError:
                    return
                continue
            # End of one direction. A tunnel is over; a plain request may still be answered.
            if source is upstream or close_on_client_eof:
                return
            del open_sockets[source]
            try:
                target.shutdown(socket.SHUT_WR)
            except OSError:
                return


class Handler(socketserver.BaseRequestHandler):
    def refuse(self, status, reason):
        body = f"netlog-proxy: {reason}\n".encode()
        head = f"HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n"
        try:
            self.request.sendall(head.encode() + body)
        except OSError:
            pass

    def handle(self):
        client = self.request
        head = b""
        while b"\r\n\r\n" not in head:
            try:
                data = client.recv(65536)
            except OSError:
                return
            if not data:
                return
            head += data
            if len(head) > HEAD_LIMIT:
                return self.refuse("431 Request Header Fields Too Large", "request head too large")
        head, _, rest = head.partition(b"\r\n\r\n")
        lines = head.split(b"\r\n")
        try:
            method, target, version = lines[0].decode("latin-1").split(" ", 2)
        except ValueError:
            return self.refuse("400 Bad Request", "malformed request line")
        headers = lines[1:]

        try:
            if method.upper() == "CONNECT":
                host, port = split_host(target, 443)
                path = None
            else:
                url = urlsplit(target)
                if url.scheme and url.scheme.lower() != "http":
                    return self.refuse("400 Bad Request", f"unsupported scheme {url.scheme}")
                authority = url.netloc.rpartition("@")[2]
                if not authority:
                    # Origin form: the host comes from the Host header.
                    hosts = [line.split(b":", 1)[1] for line in headers if line.lower().startswith(b"host:")]
                    authority = hosts[0].decode("latin-1").strip() if hosts else ""
                host, port = split_host(authority, 80)
                path = url.path or "/"
        except ValueError:
            return self.refuse("400 Bad Request", "malformed target")
        if not host:
            return self.refuse("400 Bad Request", "no host")

        if path is None:
            log(f"CONNECT {name(host, port)}")
        else:
            log(f"{method.upper()} {name(host, port)} {path}")

        try:
            upstream = socket.create_connection((host, port), timeout=CONNECT_TIMEOUT)
        except OSError as error:
            reason = error.strerror or str(error) or type(error).__name__
            log(f"FAILED {name(host, port)} {reason}", comment=True)
            return self.refuse("502 Bad Gateway", f"cannot reach {name(host, port)}: {reason}")
        upstream.settimeout(None)
        connection = Connection(name(host, port))

        with upstream:
            try:
                if path is None:
                    client.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
                else:
                    # Origin form towards the server, one request per connection: after the head the
                    # bytes are copied as they come, so bodies and streamed answers need no parsing.
                    query = f"?{url.query}" if url.query else ""
                    request = [f"{method} {path}{query} {version}".encode("latin-1")]
                    request += [line for line in headers if line.split(b":", 1)[0].strip().lower() not in HOP_HEADERS]
                    request.append(b"Connection: close")
                    request = b"\r\n".join(request) + b"\r\n\r\n"
                    upstream.sendall(request)
                    connection.sent += len(request)
                if rest:
                    upstream.sendall(rest)
                    connection.sent += len(rest)
                relay(client, upstream, path is None, connection)
            except OSError:
                pass
            finally:
                connection.close()


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True
    # The app opens dozens of connections at once (one per icon of the agent registry): with the
    # default backlog of 5 the surplus is reset, which the app sees as failed requests.
    request_queue_size = 512


def main():
    global log_file
    if len(sys.argv) != 3 or sys.argv[1] in ("-h", "--help"):
        print(__doc__, file=sys.stderr)
        return 2
    log_file = open(sys.argv[2], "a")
    server = Server(("127.0.0.1", int(sys.argv[1])), Handler)
    print(server.server_address[1], flush=True)
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        server.serve_forever()
    except (KeyboardInterrupt, SystemExit):
        pass
    finally:
        # Tunnels the client kept open: their counts would be lost with the process.
        for connection in list(open_connections):
            connection.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
