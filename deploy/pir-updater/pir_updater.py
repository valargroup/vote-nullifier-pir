#!/usr/bin/env python3
"""Host-local reconciler. Only the pinned verifier can authorize a candidate."""
import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path('/opt/pir-updater')
STATUS = Path('/var/lib/pir-updater/status.json')
SERVICE = Path('/etc/systemd/system/nullifier-query-server.service')
LOCK = Path('/run/lock/pir-update.lock')
NAME = 'nullifier-query-server.service'
DROPIN = SERVICE.parent / 'nullifier-query-server.service.d/90-pir-updater.conf'
BINARY = Path('/opt/nf-ingest/nf-server')
USER_AGENT = 'pir-updater/1'


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def durable_mkdir(path):
    if path.is_dir():
        return
    durable_mkdir(path.parent)
    path.mkdir(exist_ok=True)
    sync_directory(path.parent)


def atomic(path, data, mode=0o644):
    durable_mkdir(path.parent)
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as f:
        temp = Path(f.name)
        try:
            f.write(data)
            f.flush()
            os.fchmod(f.fileno(), mode)
            os.fsync(f.fileno())
            os.replace(temp, path)
            fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        finally:
            temp.unlink(missing_ok=True)


def save(path, value):
    atomic(path, (json.dumps(value, sort_keys=True) + '\n').encode())


def read(path, default=None):
    return json.loads(path.read_bytes()) if path.exists() else default


def run(*args, timeout=120, check=True):
    # Never print command arguments or subprocess output: host config may contain secrets.
    result = subprocess.run([str(a) for a in args], capture_output=True, timeout=timeout)
    if check and result.returncode:
        raise RuntimeError(f'{Path(str(args[0])).name} failed (exit {result.returncode})')
    return result.stdout


def fetch(url, limit=65536):
    if not url.startswith('https://'):
        raise ValueError('HTTPS required')
    with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': USER_AGENT}), timeout=30) as response:
        if not response.url.startswith('https://'):
            raise ValueError('HTTPS required after redirect')
        data = response.read(limit + 1)
        if len(data) > limit:
            raise ValueError('response exceeds size limit')
        return data


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest() if hasattr(hashlib, 'file_digest') else stream_digest(f)


def stream_digest(f):
    h = hashlib.sha256()
    for chunk in iter(lambda: f.read(1024 * 1024), b''):
        h.update(chunk)
    return h.hexdigest()


def download(urls, path, expected, max_bytes=1024 * 1024 * 1024):
    for url in urls:
        try:
            if not url.startswith('https://'):
                raise ValueError('HTTPS required')
            with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': USER_AGENT}), timeout=1800) as response, path.open('wb') as out:
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
            return
        except Exception:
            path.unlink(missing_ok=True)
    raise RuntimeError('artifact unavailable or hash mismatch from all sources')


def switch(path):
    temp = ROOT / 'current.new'
    temp.unlink(missing_ok=True)
    temp.symlink_to(path)
    os.replace(temp, ROOT / 'current')
    fd = os.open(ROOT, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def managed_dropin():
    return ('[Service]\nExecStart=\nExecStart=/opt/nf-ingest/nf-server serve --port 3000 '
            f'--pir-data-dir {ROOT}/current/data --pir-config-url= --voting-config-url=\n').encode()


def restore_dropin(encoded):
    if encoded is not None:
        atomic(DROPIN, base64.b64decode(encoded))
    elif DROPIN.exists():
        DROPIN.unlink()
        sync_directory(DROPIN.parent)


class Reconciler:
    def __init__(self):
        self.settings = read(ROOT / 'settings.json')
        self.status = read(STATUS, {})
        self.status.update(enabled=True)

    def report(self, **fields):
        self.status.update(fields)
        save(STATUS, self.status)
        print(json.dumps(fields, sort_keys=True), flush=True)

    def target(self):
        raw = fetch(self.settings['config_url'])
        att = fetch(self.settings['config_url'].rsplit('/', 1)[0] + '/pir_attestations.json')
        with tempfile.TemporaryDirectory(dir=ROOT) as folder:
            folder = Path(folder)
            (folder / 'pir.json').write_bytes(raw)
            (folder / 'pir_attestations.json').write_bytes(att)
            verified = json.loads(run(ROOT / 'verifier', 'verify-pir-update', '--scope', self.settings['scope'],
                '--zcash-network', self.settings['network'], '--config', folder / 'pir.json',
                '--attestations', folder / 'pir_attestations.json'))
        # Identity includes authenticated artifact hashes, not signature ordering.
        identity = hashlib.sha256(json.dumps(verified, sort_keys=True).encode()).hexdigest()
        return identity, verified

    def metadata(self):
        with urllib.request.urlopen('http://127.0.0.1:3000/ready', timeout=5) as r:
            if r.status != 200:
                raise RuntimeError('server not ready')
        with urllib.request.urlopen('http://127.0.0.1:3000/metadata', timeout=5) as r:
            return json.loads(r.read(65536))

    def matches(self, target):
        try:
            if 'legacy_binary_sha256' in target:
                # Only locally captured rollback records use this path. Signed
                # candidates always require exact metadata after activation.
                with urllib.request.urlopen('http://127.0.0.1:3000/ready', timeout=5) as response:
                    if response.status != 200:
                        return False
                pid = int(run('systemctl', 'show', NAME, '--property=MainPID', '--value'))
                return (pid > 0 and digest(BINARY) == target['legacy_binary_sha256'] and
                        digest(Path(f'/proc/{pid}/exe')) == target['running_exe_sha256'] and
                        digest(Path(target['data_dir']) / 'pir_root.json') == target['root_sha256'])
            m = self.metadata()
            return (m['release_tag'] == target['binary_tag'] and
                    m['snapshot_height'] == target['snapshot_height'] and
                    m['zcash_network'] == self.settings['network'])
        except Exception:
            return False

    def wait(self, target):
        deadline = time.monotonic() + self.settings.get('timeout_secs', 600)
        while time.monotonic() < deadline:
            if self.matches(target):
                return
            time.sleep(2)
        raise RuntimeError('server failed readiness or target identity check')

    def rollback(self, tx):
        run('systemctl', 'stop', NAME)
        switch(tx['previous'])
        atomic(SERVICE, base64.b64decode(tx['previous_unit']))
        if 'previous_dropin' in tx:
            restore_dropin(tx['previous_dropin'])
        run('systemctl', 'daemon-reload')
        run('systemctl', 'reset-failed', NAME, check=False)
        run('systemctl', 'start', NAME)
        self.wait(tx['previous_target'])
        (ROOT / 'transaction.json').unlink()
        sync_directory(ROOT)
        self.report(phase='rolled_back', converged=False,
                    rollbacks=self.status.get('rollbacks', 0) + 1)

    def recover(self):
        tx = read(ROOT / 'transaction.json')
        if tx:
            self.rollback(tx)

    def stage(self, identity, verified):
        cfg, p = verified['config'], verified['payload']
        path = ROOT / 'generations' / identity
        if path.exists():
            shutil.rmtree(path)
        path.mkdir(parents=True)
        sync_directory(path.parent)
        tag = cfg['binary_tag']
        arch = {'x86_64': 'amd64', 'aarch64': 'arm64', 'arm64': 'arm64'}.get(platform.machine())
        if not arch:
            raise RuntimeError('unsupported architecture')
        manifest = fetch(f"{self.settings['snapshot_base']}/snapshots/{self.settings['network']}/{cfg['snapshot_height']}/manifest.json", 1048576)
        if hashlib.sha256(manifest).hexdigest() != p['snapshot_manifest_sha256']:
            raise RuntimeError('snapshot manifest hash mismatch')
        files = json.loads(manifest)['files']
        sizes = [files[name]['size'] for name in ('tier0.bin', 'tier1.bin', 'pir_root.json')]
        if any(type(n) is not int or n < 0 for n in sizes):
            raise ValueError('invalid snapshot sizes')
        required = sum(sizes) + 1024 * 1024 * 1024
        if shutil.disk_usage(ROOT).free < required:
            raise RuntimeError('insufficient disk space for staged generation')
        base = self.settings['binary_base']
        github = f'https://github.com/valargroup/vote-nullifier-pir/releases/download/{tag}'
        download([f'{base}/nf-server-{tag}-linux-{arch}', f'{github}/nf-server-linux-{arch}'],
                 path / 'nf-server', p[f'linux_{arch}_sha256'])
        os.chmod(path / 'nf-server', 0o755)
        with (path / 'nf-server').open('rb') as binary:
            os.fsync(binary.fileno())
        # The candidate has been authenticated before its first execution.
        info = json.loads(run(path / 'nf-server', 'build-info', '--json'))
        if info['release_tag'] != tag or info.get('pir_update_protocol') != 1:
            raise RuntimeError('candidate build identity or updater protocol mismatch')
        download([f'{base}/nullifier-query-server-{tag}.service', f'{github}/nullifier-query-server.service'],
                 path / 'service', p['service_sha256'], 65536)
        run(path / 'nf-server', 'snapshot-stage', '--zcash-network', self.settings['network'],
            '--height', cfg['snapshot_height'], '--pir-data-dir', path / 'data',
            '--precomputed-base-url', self.settings['snapshot_base'],
            '--manifest-sha256', p['snapshot_manifest_sha256'], timeout=14400)
        save(path / 'target.json', cfg)
        sync_directory(path)
        sync_directory(path.parent)
        return path

    def activate(self, path):
        previous = (ROOT / 'current').resolve()
        tx = {'previous': str(previous), 'target': str(path),
              'previous_unit': base64.b64encode(SERVICE.read_bytes()).decode(),
              'previous_target': read(previous / 'target.json'),
              'previous_dropin': base64.b64encode(DROPIN.read_bytes()).decode() if DROPIN.exists() else None}
        save(ROOT / 'transaction.json', tx)
        self.report(phase='activating', converged=False)
        try:
            run('systemctl', 'stop', NAME)
            switch(path)
            atomic(SERVICE, (path / 'service').read_bytes())
            atomic(DROPIN, managed_dropin())
            run('systemctl', 'daemon-reload')
            run('systemctl', 'reset-failed', NAME, check=False)
            run('systemctl', 'start', NAME)
            self.wait(read(path / 'target.json'))
        except Exception:
            self.rollback(tx)
            raise
        save(ROOT / 'previous.json', str(previous))
        (ROOT / 'transaction.json').unlink()
        sync_directory(ROOT)
        # Only clean after readiness has committed a successful generation.
        for old in (ROOT / 'generations').iterdir():
            if old not in (previous, path):
                shutil.rmtree(old)

    def once(self, force_retry=False):
        if (ROOT / 'enrollment.json').exists():
            raise RuntimeError('incomplete enrollment; rerun the updater installer')
        self.recover()
        self.report(last_check=int(time.time()), phase='checking')
        identity, verified = self.target()
        changed = identity != self.status.get('desired_id')
        self.report(last_verified=int(time.time()), desired_id=identity, desired=verified['config'])
        if changed:
            self.report(failures=0, next_retry=0)
        current = (ROOT / 'current').resolve()
        if current.name == identity and self.matches(verified['config']):
            self.report(phase='current', converged=True, error=None)
            return
        if not force_retry and self.status.get('next_retry', 0) > time.time():
            self.report(phase='backoff', converged=False)
            return
        self.report(phase='staging', converged=False)
        # A stopped/unhealthy current generation is restarted without deleting its files.
        if current.name == identity:
            run('systemctl', 'restart', NAME)
            self.wait(verified['config'])
        else:
            path = self.stage(identity, verified)
            latest, _ = self.target()
            if latest != identity:
                shutil.rmtree(path)
                self.report(phase='target_changed', converged=False)
                return
            self.activate(path)
        self.report(phase='current', converged=True, error=None, failures=0,
                    next_retry=0, last_success=int(time.time()))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['once', 'retry-now', 'status', 'disable', 'uninstall'])
    args = parser.parse_args()
    if args.command == 'status':
        print(json.dumps(read(STATUS, {}), indent=2))
        return
    if os.geteuid() != 0:
        parser.error('root is required')
    LOCK.parent.mkdir(parents=True, exist_ok=True)
    with LOCK.open('w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print('another PIR operation holds the lock', flush=True)
            return
        if (ROOT / 'enrollment.json').exists():
            from install import recover_enrollment
            recover_enrollment()
            raise SystemExit('Interrupted enrollment restored; rerun the installer')
        updater = Reconciler()
        if args.command in ('disable', 'uninstall'):
            run('systemctl', 'disable', '--now', 'pir-updater.timer')
            updater.report(enabled=False, phase='disabled')
            if args.command == 'uninstall':
                # Materialize the active binary and retain its data path. No restart required.
                current = (ROOT / 'current').resolve()
                shutil.copy2(current / 'nf-server', '/opt/nf-ingest/nf-server.unmanaged')
                os.replace('/opt/nf-ingest/nf-server.unmanaged', '/opt/nf-ingest/nf-server')
                dropin = Path('/etc/systemd/system/nullifier-query-server.service.d/90-pir-updater.conf')
                atomic(dropin, dropin.read_bytes().replace(str(ROOT / 'current' / 'data').encode(), str(current / 'data').encode()))
                for unit in ('pir-updater.timer', 'pir-updater.service'):
                    Path('/etc/systemd/system', unit).unlink(missing_ok=True)
                (ROOT / 'settings.json').unlink()
                run('systemctl', 'daemon-reload')
            return
        try:
            updater.once(args.command == 'retry-now')
        except Exception as e:
            failures = updater.status.get('failures', 0) + 1
            # Bounded messages only. Do not expose URLs, defaults, or child stderr.
            safe = str(e) if isinstance(e, RuntimeError) else type(e).__name__
            updater.report(phase='error', converged=False, error=safe[:200], failures=failures,
                           failures_total=updater.status.get('failures_total', 0) + 1,
                           next_retry=int(time.time()) + min(3600, 60 * 2 ** min(failures - 1, 6)))
            raise SystemExit(1)


if __name__ == '__main__':
    main()
