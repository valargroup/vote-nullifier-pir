#!/usr/bin/env python3
"""Explicit enrollment of an existing, compatible start_pir installation."""
import argparse
import base64
import fcntl
import json
import os
from pathlib import Path
import time
import subprocess
from urllib.parse import urlparse
from pir_updater import (ROOT, SERVICE, LOCK, BINARY, DROPIN, Reconciler, atomic, save,
                         run, switch, sync_directory, read, digest, restore_dropin)


def defaults(path):
    result = {}
    if path.exists():
        for line in path.read_text().splitlines():
            if not line or line.startswith('#'):
                continue
            key, sep, value = line.partition('=')
            if not sep or value.startswith(('"', "'")) or '\\' in value:
                raise RuntimeError('unsupported environment file syntax; use plain KEY=value entries')
            result[key] = value
    return result


def recover_enrollment():
    journal = ROOT / 'enrollment.json'
    if not journal.exists():
        return
    tx = json.loads(journal.read_bytes())
    # Preserve the old service until staging succeeds. After activation begins,
    # recover its exact executable/unit and readiness, even without /metadata.
    if tx.get('activation_started', True):
        run('systemctl', 'stop', 'nullifier-query-server.service')
        atomic(BINARY, Path(tx['binary_backup']).read_bytes(), 0o755)
        if 'previous_unit' in tx:
            atomic(SERVICE, base64.b64decode(tx['previous_unit']))
        restore_dropin(tx.get('previous_dropin'))
        run('systemctl', 'daemon-reload')
        run('systemctl', 'reset-failed', 'nullifier-query-server.service', check=False)
        run('systemctl', 'start', 'nullifier-query-server.service')
        target = tx.get('previous_target')
        if target:
            recovery = Reconciler()
            recovery.settings = {'timeout_secs': tx.get('timeout_secs', 600)}
            recovery.wait(target)
    # Keep the timer and journal until restoration succeeds, so boot recovery
    # can retry a failed restore. All cleanup operations are idempotent.
    run('systemctl', 'disable', '--now', 'pir-updater.timer', check=False)
    for name in ('pir-updater.service', 'pir-updater.timer'):
        Path('/etc/systemd/system', name).unlink(missing_ok=True)
    BINARY.with_suffix('.managed').unlink(missing_ok=True)
    (ROOT / 'transaction.json').unlink(missing_ok=True)
    run('systemctl', 'daemon-reload')
    sync_directory(Path('/etc/systemd/system'))
    sync_directory(BINARY.parent)
    # The journal retains everything needed even after settings are removed.
    (ROOT / 'settings.json').unlink(missing_ok=True)
    sync_directory(ROOT)
    journal.unlink()
    sync_directory(ROOT)
    (ROOT / 'current').unlink(missing_ok=True)
    sync_directory(ROOT)
    print('Original PIR service preserved or restored; rerun the installer to retry.')


def install(timeout):
    if os.geteuid() or os.uname().sysname != 'Linux':
        raise RuntimeError('Linux root installation required')
    sync_directory(ROOT.parent)
    for file in (ROOT / "install.py", ROOT / "pir_updater.py", ROOT / "verifier"):
        with file.open("rb") as source:
            os.fsync(source.fileno())
    sync_directory(ROOT)
    recover_enrollment()
    settings = ROOT / 'settings.json'
    if settings.exists():
        updater = Reconciler()
        updater.once(force_retry=True)
        if not updater.status.get('converged'):
            raise RuntimeError('target changed during staging; rerun the installer')
        run('systemctl', 'enable', '--now', 'pir-updater.timer')
        print('Signed target is ready; automatic updates enabled.')
        return
    env = defaults(Path('/etc/default/nf-server'))
    env.update(defaults(Path('/opt/nf-ingest/.env')))
    network = env.get('SVOTE_ZCASH_NETWORK')
    if network not in ('main', 'test'):
        raise RuntimeError('installed Zcash network must be main or test')
    url = env.get('SVOTE_PIR_CONFIG_URL')
    if url is None:
        legacy = env.get('SVOTE_PIR_VOTING_CONFIG_URL', '')
        if '/stage/' in legacy:
            url = 'https://voting.valargroup.dev/stage/pir.json'
        elif '/prod/' in legacy or legacy == 'https://voting.valargroup.org/static-voting-config.json':
            url = 'https://voting.valargroup.dev/prod/pir.json'
    parsed = urlparse(url or '')
    scope = parsed.path.split('/')[-2] if '/' in parsed.path else ''
    if parsed.scheme != 'https' or scope not in ('prod', 'stage') or not parsed.path.endswith('/pir.json') or parsed.query or parsed.fragment or parsed.username:
        raise RuntimeError('configure an explicit HTTPS /prod/pir.json or /stage/pir.json URL before enrollment')
    if env.get('SVOTE_PIR_FORCE_SNAPSHOT_HEIGHT'):
        raise RuntimeError('remove the forced snapshot override before enrollment')
    if b'ExecStart=/opt/nf-ingest/nf-server serve --port 3000\n' not in SERVICE.read_bytes():
        raise RuntimeError('unsupported custom ExecStart; restore the release unit before enrollment')
    dropins = run('systemctl', 'show', '--property=DropInPaths', '--value', 'nullifier-query-server.service').strip()
    if dropins:
        raise RuntimeError('custom systemd overrides require manual migration before enrollment')
    # The installer provides a checksum-verified, separately pinned verifier.
    info = json.loads(run(ROOT / 'verifier', 'build-info', '--json'))
    if info.get('pir_update_protocol') != 1:
        raise RuntimeError('bootstrap verifier does not support PIR updates')
    if not BINARY.is_file() or BINARY.is_symlink():
        raise RuntimeError('expected an unmanaged regular nf-server executable')
    data = Path(env.get('SVOTE_PIR_DATA_DIR', f'/opt/nf-ingest/pir-data/{network}')).resolve()
    root = read(data / 'pir_root.json', {})
    height = root.get('height')
    if root.get('zcash_network') != network or type(height) is not int or height <= 0:
        raise RuntimeError('configured snapshot has an invalid network or height')
    pid = int(run('systemctl', 'show', 'nullifier-query-server.service', '--property=MainPID', '--value'))
    if pid <= 0:
        raise RuntimeError('existing PIR service must be running')
    installed_hash = digest(BINARY)
    running_hash = digest(Path(f'/proc/{pid}/exe'))
    if installed_hash != running_hash:
        raise RuntimeError('running and installed server executable differ; reconcile before enrollment')
    previous_target = {'legacy_binary_sha256': installed_hash,
                       'running_exe_sha256': running_hash,
                       'root_sha256': digest(data / 'pir_root.json'),
                       'data_dir': str(data), 'snapshot_height': height}
    initial = ROOT / 'generations' / 'initial'
    initial.mkdir(parents=True, exist_ok=True)
    sync_directory(initial.parent)
    sync_directory(ROOT)
    atomic(initial / 'nf-server', BINARY.read_bytes(), 0o755)
    initial_data = initial / 'data'
    initial_data.unlink(missing_ok=True)
    initial_data.symlink_to(data)
    save(initial / 'target.json', previous_target)
    tx = {'binary_backup': str(initial / 'nf-server'), 'previous_target': previous_target,
          'previous_unit': base64.b64encode(SERVICE.read_bytes()).decode(),
          'previous_dropin': None, 'activation_started': False, 'timeout_secs': timeout}
    save(ROOT / 'enrollment.json', tx)
    save(settings, {'scope':scope, 'network':network, 'config_url':url,
        'snapshot_base':env.get('SVOTE_PIR_PRECOMPUTED_BASE_URL', 'https://shielded-vote.nyc3.digitaloceanspaces.com').rstrip('/'),
        'binary_base':'https://shielded-vote.nyc3.digitaloceanspaces.com/binaries/vote-pir', 'timeout_secs':timeout})
    updater = Reconciler()
    if not updater.matches(previous_target):
        raise RuntimeError('existing PIR service is not ready')
    updater.report(phase='checking', converged=False, last_check=int(time.time()))
    identity, verified = updater.target()
    updater.report(phase='staging', desired_id=identity, desired=verified['config'],
                   last_verified=int(time.time()))
    candidate = updater.stage(identity, verified)
    latest, _ = updater.target()
    if latest != identity:
        raise RuntimeError('signed target changed during staging; rerun the installer')
    if not updater.matches(previous_target):
        raise RuntimeError('original server changed during staging')
    switch(initial)
    atomic(Path('/etc/systemd/system/pir-updater.service'), b'''[Unit]
Description=Apply coordinator-authorized PIR updates
Wants=network-online.target
After=network-online.target
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /opt/pir-updater/pir_updater.py once
TimeoutStartSec=6h
UMask=0022
''')
    atomic(Path('/etc/systemd/system/pir-updater.timer'), b'''[Unit]
Description=Check coordinator-authorized PIR updates
[Timer]
OnBootSec=60
OnUnitInactiveSec=60
RandomizedDelaySec=30
[Install]
WantedBy=timers.target
''')
    run('systemctl', 'daemon-reload')
    # Recovery is scheduled before the first serving-path mutation. The shared
    # lock keeps it from racing this installer; after a crash it restores legacy.
    run('systemctl', 'enable', '--now', 'pir-updater.timer')
    tx['activation_started'] = True
    save(ROOT / 'enrollment.json', tx)
    link = BINARY.with_suffix('.managed')
    link.unlink(missing_ok=True)
    link.symlink_to(ROOT / 'current' / 'nf-server')
    os.replace(link, BINARY)
    sync_directory(BINARY.parent)
    updater.activate(candidate)
    updater.report(phase='current', converged=True, error=None, failures=0,
                   next_retry=0, last_success=int(time.time()))
    (ROOT / 'enrollment.json').unlink()
    sync_directory(ROOT)
    print('Signed target is ready; automatic updates enabled.')



if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--timeout-secs', type=int, default=600)
    parser.add_argument('--lock-fd', type=int)
    args = parser.parse_args()
    if args.timeout_secs <= 0:
        parser.error('timeout must be positive')
    LOCK.parent.mkdir(parents=True, exist_ok=True)
    lock = os.fdopen(os.dup(args.lock_fd), 'w') if args.lock_fd is not None else LOCK.open('w')
    with lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            install(args.timeout_secs)
        except Exception:
            recover_enrollment()
            raise
