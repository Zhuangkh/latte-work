#!/usr/bin/env python3
"""macOS-only native favicon transport tests against a disposable localhost fixture."""
import http.server
from pathlib import Path
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parent.parent
requests = []
icon = b'<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"/>'


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_GET(self):
        requests.append(dict(self.headers))
        if self.path.startswith('/redirect/'):
            remaining = int(self.path.rsplit('/', 1)[1])
            self.send_response(302)
            self.send_header('Location', '/icon' if remaining == 1 else f'/redirect/{remaining - 1}')
            self.end_headers()
            return
        if self.path == '/auth':
            self.send_response(401)
            self.send_header('WWW-Authenticate', 'Basic realm="isolated-test"')
            self.end_headers()
            return
        if self.path == '/slow':
            time.sleep(4)
        body = b'A' * 65537 if self.path in ('/large', '/stream-large') else icon
        self.send_response(200)
        self.send_header('Content-Type', 'text/html' if self.path == '/html' else 'image/svg+xml')
        self.send_header('Set-Cookie', 'fixture=do-not-forward; Path=/')
        if self.path != '/stream-large':
            self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory(prefix='latte-native-icons-') as directory:
        binary = str(Path(directory) / 'test-icons')
        subprocess.run(['xcrun', 'clang', '-fobjc-arc', '-Wall', '-Wextra', '-Werror',
                        '-Wno-unused-parameter', '-framework', 'AppKit', '-framework', 'WebKit',
                        '-framework', 'Security', str(root / 'apps/desktop/src-tauri/tests/macos_site_icon.m'),
                        '-o', binary], check=True)
        subprocess.run([binary, f'http://127.0.0.1:{server.server_port}'], check=True, timeout=30)
    assert not any(any(key.lower() in ('cookie', 'authorization', 'referer') for key in headers) for headers in requests)
    print('PASS: no cookies, saved credentials or referrer sent')
finally:
    server.shutdown()
    server.server_close()
