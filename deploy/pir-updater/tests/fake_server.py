#!/usr/bin/python3
"""Test-only process with real HTTP and systemd lifecycle, no PIR computation."""
import json
from pathlib import Path
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler
folder=Path('/proc/self/exe').resolve().parent
target=json.loads((folder/'target.json').read_bytes())
legacy = (folder/'legacy').exists() or 'legacy_binary_sha256' in target
legacy_flags = ['--pir-data-dir', '--voting-config-url']
if not Path('/legacy-without-pir-config').exists(): legacy_flags.append('--pir-config-url')
if '--help' in sys.argv:
    print(' '.join(legacy_flags));raise SystemExit()
if legacy and '--pir-config-url' not in legacy_flags and '--pir-config-url=' in sys.argv:
    raise SystemExit('unsupported legacy option')
if 'build-info' in sys.argv and legacy: raise SystemExit(2)
if 'build-info' in sys.argv:
    print(json.dumps({'release_tag':target['binary_tag'],'pir_update_protocol':1}));raise SystemExit()
if (folder/'fail').exists():raise SystemExit(1)
if legacy and Path('/remote-snapshot.json').exists():
    # Model legacy bootstrap: live discovery can change the original dataset or
    # prevent startup when the config endpoint is unavailable.
    disabled = '--voting-config-url=' in sys.argv and (
        '--pir-config-url' not in legacy_flags or '--pir-config-url=' in sys.argv)
    if not disabled:
        remote = json.loads(Path('/remote-snapshot.json').read_bytes())
        if remote is None: raise SystemExit('config endpoint unavailable')
        Path('/opt/nf-ingest/pir-data/main/pir_root.json').write_text(json.dumps(remote))
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
