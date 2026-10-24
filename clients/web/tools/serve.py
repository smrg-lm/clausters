#!/usr/bin/env python3
"""Serve the web package, with the headers a page needs to share memory.

    python3 tools/serve.py [port] [--isolated]     # from clients/web/

An editing application in a page (the audio editor) runs over shared memory,
which a browser grants only to a page that is **cross-origin isolated**: one
served with `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`. Every response here carries the
embedder policy and `Cross-Origin-Resource-Policy: same-origin`, so a script
or a worker an isolated page loads is one it may run; the opener policy, which
is what isolates a page, goes on a document whose query names `isolated`, or
on every document with `--isolated`. A page served without it is an ordinary
page, the kind a component embeds in, and loads the ordinary host.

The suite (`test.sh`) serves with this and reads its access log, written to
stderr in `http.server`'s own format.
"""

import functools
import http.server
import sys
from urllib.parse import urlsplit

ISOLATE_ALL = "--isolated" in sys.argv[1:]


class Handler(http.server.SimpleHTTPRequestHandler):
    def end_headers(self):
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        query = urlsplit(self.path).query
        if ISOLATE_ALL or "isolated" in query.split("&") or "isolated=1" in query.split("&"):
            self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        super().end_headers()


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    port = int(args[0]) if args else 8000
    bind = args[1] if len(args) > 1 else "127.0.0.1"
    handler = functools.partial(Handler, directory=".")
    http.server.ThreadingHTTPServer((bind, port), handler).serve_forever()


if __name__ == "__main__":
    main()
