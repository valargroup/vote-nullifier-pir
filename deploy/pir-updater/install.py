#!/usr/bin/env python3
"""Explicit enrollment of an existing, compatible start_pir installation."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import subprocess
from urllib.parse import urlparse
from pir_updater import ROOT, SERVICE, LOCK, atomic, save, run, switch, sync_directory


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
    # A retry always restores the pre-enrollment executable before trying again.
    subprocess.run(['systemctl', 'disable', '--now', 'pir-updater.timer'], capture_output=True, timeout=120)
    original = Path(tx['binary_backup'])
    atomic(Path('/opt/nf-ingest/nf-server'), original.read_bytes(), 0o755)
    Path('/etc/systemd/system/nullifier-query-server.service.d/90-pir-updater.conf').unlink(missing_ok=True)
    for name in ('pir-updater.service', 'pir-updater.timer'):
        Path('/etc/systemd/system', name).unlink(missing_ok=True)
    Path('/opt/nf-ingest/nf-server.managed').unlink(missing_ok=True)
    (ROOT / 'settings.json').unlink(missing_ok=True)
    run('systemctl', 'daemon-reload')
    for directory in (Path('/etc/systemd/system/nullifier-query-server.service.d'),
                      Path('/etc/systemd/system'), Path('/opt/nf-ingest')):
        if directory.exists():
            sync_directory(directory)
    journal.unlink()
    sync_directory(ROOT)
    shutil.rmtree(ROOT / 'generations' / 'initial')
    (ROOT / 'current').unlink(missing_ok=True)
    sync_directory(ROOT / 'generations')
    sync_directory(ROOT)


def install(timeout):
    if os.geteuid() or os.uname().sysname != 'Linux':
        raise RuntimeError('Linux root installation required')
    sync_directory(ROOT.parent)
    for file in (ROOT / "install.py", ROOT / "pir_updater.py"):
        with file.open("rb") as source:
            os.fsync(source.fileno())
    sync_directory(ROOT)
    recover_enrollment()
    settings = ROOT / 'settings.json'
    if settings.exists():
        print('Already enrolled. Use systemctl enable --now pir-updater.timer to re-enable.')
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
    binary = Path('/opt/nf-ingest/nf-server')
    info = json.loads(run(binary, 'build-info', '--json'))
    if info.get('pir_update_protocol') != 1:
        raise RuntimeError('run update_pir.sh with a compatible release before enrollment')
    import urllib.request
    with urllib.request.urlopen('http://127.0.0.1:3000/ready', timeout=5):
        pass
    with urllib.request.urlopen('http://127.0.0.1:3000/metadata', timeout=5) as response:
        metadata = json.loads(response.read(65536))
    if metadata['release_tag'] != info['release_tag'] or metadata['zcash_network'] != network:
        raise RuntimeError('running and installed server identity differ')
    data = Path(env.get('SVOTE_PIR_DATA_DIR', f'/opt/nf-ingest/pir-data/{network}')).resolve()
    if not (data / 'pir_root.json').is_file():
        raise RuntimeError('configured data directory has no snapshot')
    initial = ROOT / 'generations' / 'initial'
    if initial.exists():
        shutil.rmtree(initial)
    initial.mkdir(parents=True, exist_ok=True)
    sync_directory(initial.parent)
    sync_directory(ROOT)
    atomic(initial / 'nf-server', binary.read_bytes(), 0o755)
    atomic(ROOT / 'verifier', binary.read_bytes(), 0o755)
    save(ROOT / 'enrollment.json', {'binary_backup':str(initial / 'nf-server')})
    (initial / 'data').symlink_to(data)
    save(initial / 'target.json', {'binary_tag':info['release_tag'], 'snapshot_height':metadata['snapshot_height']})
    switch(initial)
    link = binary.with_suffix('.managed')
    link.unlink(missing_ok=True)
    link.symlink_to(ROOT / 'current' / 'nf-server')
    os.replace(link, binary)
    sync_directory(binary.parent)
    atomic(Path('/etc/systemd/system/nullifier-query-server.service.d/90-pir-updater.conf'),
        ('[Service]\nExecStart=\nExecStart=/opt/nf-ingest/nf-server serve --port 3000 '
         f'--pir-data-dir {ROOT}/current/data --pir-config-url= --voting-config-url=\n').encode())
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
    save(settings, {'scope':scope, 'network':network, 'config_url':url,
        'snapshot_base':env.get('SVOTE_PIR_PRECOMPUTED_BASE_URL', 'https://shielded-vote.nyc3.digitaloceanspaces.com').rstrip('/'),
        'binary_base':'https://shielded-vote.nyc3.digitaloceanspaces.com/binaries/vote-pir', 'timeout_secs':timeout})
    run('systemctl', 'daemon-reload')
    run('systemctl', 'enable', '--now', 'pir-updater.timer')
    (ROOT / 'enrollment.json').unlink()
    sync_directory(ROOT)
    print('Enrolled. The current server keeps running; updates require coordinator authorization.')


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
