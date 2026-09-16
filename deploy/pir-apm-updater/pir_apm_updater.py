#!/usr/bin/env python3
"""Host-local pir-apm reconciler.

Follows the same `binary_tag` as the serving updater so the monitoring sidecar
cannot drift from the server it scrapes. It deliberately does not verify the
coordinator signature: pir-apm never answers a PIR query, so it sits outside
the coordinator's signing scope. Integrity comes from the release's SHA256SUMS,
which is the same anchor the fleet deploy has always used for this binary.

Shares no state, lock, or code path with pir_updater. It never reads
/opt/pir-updater, never touches nf-server, its unit, its drop-in or its
generations, and never takes the serving lock, so a fault here cannot cost
query availability. A host without this service installed is simply
unmanaged; integrators never receive a sidecar.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path('/opt/pir-apm-updater')
SETTINGS = ROOT / 'settings.json'
STATE = Path('/var/lib/pir-apm-updater/state.json')
LOCK = Path('/run/lock/pir-apm-update.lock')
BINARY = Path('/opt/nf-ingest/pir-apm')
SERVICE = Path('/etc/systemd/system/pir-apm.service')
DEFAULTS = Path('/etc/default/pir-apm')
NAME = 'pir-apm.service'
REPO = 'valargroup/vote-nullifier-pir'
ASSET = 'pir-apm-linux-amd64'
UNIT_ASSET = 'pir-apm.service'
USER_AGENT = 'pir-apm-updater/1'
# binary_tag arrives unauthenticated and is interpolated into a download URL,
# so it is matched exactly rather than trusted as text.
TAG = re.compile(r'^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$')
MAX_BINARY_BYTES = 128 * 1024 * 1024


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic(path, data, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as f:
        temp = Path(f.name)
        try:
            f.write(data)
            f.flush()
            os.fchmod(f.fileno(), mode)
            os.fsync(f.fileno())
            os.replace(temp, path)
            sync_directory(path.parent)
        finally:
            temp.unlink(missing_ok=True)


def save(path, value):
    atomic(path, json.dumps(value, sort_keys=True).encode() + b'\n')


def read(path, default=None):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, ValueError):
        return default


def run(*args, timeout=120, check=True):
    result = subprocess.run([str(a) for a in args], capture_output=True, timeout=timeout)
    if check and result.returncode != 0:
        raise RuntimeError(f'{args[0]} failed: {result.stderr.decode(errors="replace").strip()}')
    return result.stdout


def fetch(url, limit=65536):
    if not url.startswith('https://'):
        raise ValueError('HTTPS required')
    request = urllib.request.Request(url, headers={'User-Agent': USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        if not response.url.startswith('https://'):
            raise ValueError('HTTPS required after redirect')
        return response.read(limit)


def download(url, path, expected, max_bytes=MAX_BINARY_BYTES):
    if not url.startswith('https://'):
        raise ValueError('HTTPS required')
    request = urllib.request.Request(url, headers={'User-Agent': USER_AGENT})
    try:
        with urllib.request.urlopen(request, timeout=900) as response, path.open('wb') as out:
            if not response.url.startswith('https://'):
                raise ValueError('HTTPS required after redirect')
            total = 0
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                if total > max_bytes:
                    raise ValueError('artifact too large')
                out.write(chunk)
            out.flush()
            os.fsync(out.fileno())
        if digest(path) != expected:
            raise ValueError('artifact hash mismatch')
    except Exception:
        path.unlink(missing_ok=True)
        raise


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        while chunk := f.read(1024 * 1024):
            h.update(chunk)
    return h.hexdigest()


def parse_sums(text):
    """Map asset name to digest from a SHA256SUMS body."""
    sums = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and re.fullmatch(r'[0-9a-f]{64}', parts[0]):
            sums[parts[1].lstrip('*')] = parts[0]
    return sums


def listen_address():
    """Where pir-apm serves, read from its environment file but never written."""
    try:
        for line in DEFAULTS.read_text().splitlines():
            key, _, value = line.partition('=')
            if key.strip() == 'PIR_APM_LISTEN' and value.strip():
                host, _, port = value.strip().rpartition(':')
                return host or '127.0.0.1', int(port)
    except (FileNotFoundError, ValueError):
        pass
    return '127.0.0.1', 3002


class Updater:
    def __init__(self):
        self.settings = read(SETTINGS) or {}
        self.state = read(STATE, {}) or {}

    def report(self, **fields):
        self.state.update(fields)
        save(STATE, self.state)
        print(json.dumps(fields, sort_keys=True), flush=True)

    def desired_tag(self):
        """Resolve binary_tag from pir.json without verifying its signature."""
        url = self.settings.get('config_url')
        if not url:
            raise RuntimeError('config_url is not configured')
        config = json.loads(fetch(url))
        tag = config.get('binary_tag')
        if tag is None:
            # Snapshot-only configuration predates binary_tag; leave the
            # installed sidecar exactly as it is.
            return None
        if not isinstance(tag, str) or not TAG.fullmatch(tag):
            raise RuntimeError(f'refusing malformed binary_tag: {tag!r}')
        return tag

    def release_url(self, tag, asset):
        return f'https://github.com/{REPO}/releases/download/{tag}/{asset}'

    def stage(self, tag):
        """Fetch and verify the sidecar for `tag` without installing it."""
        staged = ROOT / 'staged'
        if staged.exists():
            for leftover in staged.iterdir():
                leftover.unlink()
        staged.mkdir(parents=True, exist_ok=True)
        sums = parse_sums(fetch(self.release_url(tag, 'SHA256SUMS'), 1048576).decode())
        for asset in (ASSET, UNIT_ASSET):
            if asset not in sums:
                # Fail closed: an unlisted asset is never installed unverified.
                raise RuntimeError(f'{asset} is absent from SHA256SUMS for {tag}')
        binary, unit = staged / ASSET, staged / UNIT_ASSET
        download(self.release_url(tag, ASSET), binary, sums[ASSET])
        os.chmod(binary, 0o755)
        download(self.release_url(tag, UNIT_ASSET), unit, sums[UNIT_ASSET], 65536)
        # A verified artifact can still be unusable on this host. Prove it runs
        # before it replaces a working sidecar.
        run(binary, '--help', timeout=30)
        return binary, unit

    def healthy(self, deadline=30):
        host, port = listen_address()
        end = time.monotonic() + deadline
        while time.monotonic() < end:
            if run('systemctl', 'is-active', NAME, check=False).strip() == b'active':
                try:
                    with socket.create_connection((host, port), timeout=5):
                        return True
                except OSError:
                    pass
            time.sleep(1)
        return False

    def activate(self, tag, binary, unit):
        previous_binary = BINARY.read_bytes() if BINARY.exists() else None
        previous_unit = SERVICE.read_bytes() if SERVICE.exists() else None
        try:
            run('systemctl', 'stop', NAME, check=False)
            atomic(BINARY, binary.read_bytes(), 0o755)
            atomic(SERVICE, unit.read_bytes())
            run('systemctl', 'daemon-reload')
            run('systemctl', 'reset-failed', NAME, check=False)
            run('systemctl', 'start', NAME)
            if not self.healthy():
                raise RuntimeError('sidecar did not become healthy')
        except Exception:
            self.rollback(previous_binary, previous_unit)
            raise
        self.report(installed_tag=tag, error=None, last_success=int(time.time()))

    def rollback(self, previous_binary, previous_unit):
        if previous_binary is not None:
            atomic(BINARY, previous_binary, 0o755)
        if previous_unit is not None:
            atomic(SERVICE, previous_unit)
        run('systemctl', 'daemon-reload', check=False)
        run('systemctl', 'start', NAME, check=False)

    def once(self):
        self.report(last_check=int(time.time()))
        tag = self.desired_tag()
        if tag is None:
            return
        # The pointer is followed faithfully in both directions so a signed
        # rollback of nf-server carries the sidecar with it.
        if tag == self.state.get('installed_tag') and BINARY.exists():
            return
        try:
            binary, unit = self.stage(tag)
            self.activate(tag, binary, unit)
        except Exception as error:
            self.report(error=str(error), desired_tag=tag)
            raise


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--once', action='store_true')
    parser.parse_args()
    LOCK.parent.mkdir(parents=True, exist_ok=True)
    with LOCK.open('w') as lock:
        # Never the serving lock: this must not contend with nf-server updates.
        fcntl.flock(lock, fcntl.LOCK_EX)
        Updater().once()


if __name__ == '__main__':
    main()
