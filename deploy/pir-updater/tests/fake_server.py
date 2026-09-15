#!/usr/bin/python3
"""Test-only process with real HTTP and systemd lifecycle, no PIR computation."""
import json
from pathlib import Path
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler
folder=Path('/proc/self/exe').resolve().parent
target=json.loads((folder/'target.json').read_bytes())
legacy = (folder/'legacy').exists() or 'legacy_binary_sha256' in target
if 'build-info' in sys.argv and legacy: raise SystemExit(2)
if 'build-info' in sys.argv:
    print(json.dumps({'release_tag':target['binary_tag'],'pir_update_protocol':1}));raise SystemExit()
if (folder/'fail').exists():raise SystemExit(1)
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/metadata' and legacy:
            self.send_response(404);self.end_headers();return
        self.send_response(200);self.end_headers()
        if self.path=='/metadata':self.wfile.write(json.dumps({'release_tag':target['binary_tag'],'snapshot_height':target['snapshot_height'],'zcash_network':'main'}).encode())
        else:self.wfile.write(b'{"status":"ok"}')
    def log_message(self,*args):pass
HTTPServer.allow_reuse_address=True
HTTPServer(('127.0.0.1',3000),Handler).serve_forever()
