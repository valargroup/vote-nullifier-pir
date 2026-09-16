#!/usr/bin/python3
"""Real systemd migration from a legacy process; downloads use local fixtures."""
import base64
import fcntl
import json
import os
from pathlib import Path
import sys
import time
import urllib.request
from unittest.mock import patch
sys.path.insert(0, '/opt/pir-updater')
import pir_updater as u
import install

assert Path('/run/systemd/system').is_dir()
# Own the operation lock just as the installer does. The timer cannot race tests.
lock = u.LOCK.open('w')
fcntl.flock(lock, fcntl.LOCK_EX)
data = Path('/opt/nf-ingest/pir-data/main')
data.mkdir(parents=True)
u.save(data/'pir_root.json', {'height':3484440, 'zcash_network':'main'})
original_root = (data/'pir_root.json').read_bytes()
# Use a real native executable so /proc identity checks are exercised. Python
# interprets /serve, while each executable's folder supplies the fixture target.
u.atomic(Path('/serve'), Path('/opt/pir-updater/tests/fake_server.py').read_bytes())
original = Path(sys.executable).read_bytes()
u.atomic(u.BINARY, original, 0o755)
u.save(u.BINARY.parent/'target.json', {'binary_tag':'v1','snapshot_height':3484440})
(u.BINARY.parent/'legacy').touch()
u.atomic(Path('/etc/default/nf-server'), b'SVOTE_ZCASH_NETWORK=main\nSVOTE_PIR_CONFIG_URL=https://example.com/prod/pir.json\nSVOTE_PIR_DATA_DIR=/opt/nf-ingest/pir-data/main\n')
unit = b'[Unit]\nDescription=PIR fixture\n[Service]\nExecStart=/opt/nf-ingest/nf-server serve --port 3000\nRestart=no\n[Install]\nWantedBy=multi-user.target\n'
u.atomic(u.SERVICE, unit)
# Bootstrap verifier is independent of the legacy executable and target version.
u.atomic(u.ROOT/'verifier', b'#!/bin/sh\nprintf \'{"release_tag":"v-bootstrap","pir_update_protocol":1}\\n\'\n', 0o755)
u.run('systemctl','daemon-reload');u.run('systemctl','start',u.NAME)
for _ in range(40):
    try:
        urllib.request.urlopen('http://127.0.0.1:3000/ready').close();break
    except Exception:time.sleep(.1)
else:raise AssertionError('legacy server failed readiness')
try:urllib.request.urlopen('http://127.0.0.1:3000/metadata')
except urllib.error.HTTPError as e:assert e.code == 404
else:raise AssertionError('fixture must not support metadata')

cfg = {'binary_tag':'v2','snapshot_height':3484450}
def target(self):return 'good', {'config':cfg}
def stage(self, identity, verified):
    path = u.ROOT/'generations'/identity
    path.mkdir(exist_ok=True);(path/'data').mkdir(exist_ok=True)
    u.atomic(path/'nf-server', original, 0o755)
    u.atomic(path/'service', unit)
    u.save(path/'target.json', verified['config'])
    (path/'fail').unlink(missing_ok=True)
    return path

def legacy_restored():
    assert not u.BINARY.is_symlink()
    assert u.BINARY.read_bytes() == original
    assert u.SERVICE.read_bytes() == unit
    if u.DROPIN.exists():
        assert u.DROPIN.read_bytes() == u.legacy_dropin(u.BINARY, data)
    assert (data/'pir_root.json').read_bytes() == original_root
    assert not (u.ROOT/'settings.json').exists()
    assert not (u.ROOT/'enrollment.json').exists()
    urllib.request.urlopen('http://127.0.0.1:3000/ready').close()

def attempt():
    try:install.install(3)
    except Exception:
        install.recover_enrollment();raise

with patch.object(u.Reconciler,'target',target), patch.object(u.Reconciler,'stage',stage):
    # Only the exact updater-owned recovery override is eligible for a retry.
    for override in (u.DROPIN, u.DROPIN.parent/'10-custom.conf'):
        u.atomic(override, b'[Service]\nEnvironment=CUSTOM_OVERRIDE=1\n')
        u.run('systemctl','daemon-reload')
        with patch.object(u.Reconciler,'target') as proposal:
            try:install.install(3)
            except RuntimeError as e:assert 'custom systemd overrides' in str(e)
            else:raise AssertionError('custom override accepted')
            proposal.assert_not_called()
        override.unlink()
        u.run('systemctl','daemon-reload')

    # A file replaced while the old executable is running cannot be a rollback baseline.
    u.atomic(u.BINARY, original + b'changed on disk', 0o755)
    with patch.object(u.Reconciler,'target') as proposal:
        try:install.install(3)
        except RuntimeError as e:assert 'running and installed' in str(e)
        else:raise AssertionError('mismatched executable accepted')
        proposal.assert_not_called()
    assert not (u.ROOT/'enrollment.json').exists()
    u.atomic(u.BINARY, original, 0o755)
    # Invalid authorization or failed downloads must not touch the live process.
    pid = u.run('systemctl','show',u.NAME,'--property=MainPID','--value')
    for method in ('target', 'stage'):
        with patch.object(u.Reconciler,method,side_effect=RuntimeError('rejected')):
            try:attempt()
            except RuntimeError:pass
            else:raise AssertionError('failure expected')
        legacy_restored()
        assert u.run('systemctl','show',u.NAME,'--property=MainPID','--value') == pid

    with patch.object(u.Reconciler,'target',side_effect=[('good',{'config':cfg}),('changed',{'config':cfg})]):
        try:attempt()
        except RuntimeError as e:assert 'changed during staging' in str(e)
        else:raise AssertionError('changed target accepted')
    legacy_restored()
    assert u.run('systemctl','show',u.NAME,'--property=MainPID','--value') == pid

    # Timer enable failure is still before any serving-path mutation.
    real_run = install.run
    def failed_enable(*args, **kwargs):
        if args[:2] == ('systemctl','enable'):raise RuntimeError('enable failed')
        return real_run(*args,**kwargs)
    with patch.object(install,'run',failed_enable):
        try:attempt()
        except RuntimeError:pass
        else:raise AssertionError('enable failure expected')
    legacy_restored()

    # Candidate cannot become ready; restore a legacy server without metadata.
    def failed_stage(self,*args):
        path=stage(self,*args);(path/'fail').touch();return path
    for remote in ({'height':3484450, 'zcash_network':'main'}, None):
        u.save(Path('/remote-snapshot.json'), remote)
        with patch.object(u.Reconciler,'stage',failed_stage):
            try:attempt()
            except RuntimeError:pass
            else:raise AssertionError('activation failure expected')
        legacy_restored()
        assert u.DROPIN.exists()
        # The recovery pin must survive later service restarts too.
        u.run('systemctl','restart',u.NAME)
        recovery=u.Reconciler();recovery.settings={'timeout_secs':3}
        recovery.wait(u.read(u.ROOT/'generations/initial/target.json'))
        legacy_restored()

    # An older binary without --pir-config-url must receive only supported flags.
    Path('/legacy-without-pir-config').touch()
    u.atomic(u.DROPIN, u.legacy_dropin(u.BINARY, data))
    with patch.object(u.Reconciler,'stage',failed_stage):
        try:attempt()
        except RuntimeError:pass
        else:raise AssertionError('activation failure expected')
    legacy_restored()
    assert b'--pir-config-url=' not in u.DROPIN.read_bytes()

    # Power loss before executable replacement: on reboot the old unit may
    # start before timer recovery. It must already have the local snapshot pin.
    replace = os.replace
    def interrupted_replace(source, destination):
        if Path(source) == u.BINARY.with_suffix('.managed'):
            raise KeyboardInterrupt('power loss before executable replacement')
        return replace(source, destination)
    with patch.object(os, 'replace', interrupted_replace):
        try:install.install(3)
        except KeyboardInterrupt:pass
        else:raise AssertionError('interruption expected')
    u.run('systemctl','restart',u.NAME)
    recovery=u.Reconciler();recovery.settings={'timeout_secs':3}
    recovery.wait(u.read(u.ROOT/'generations/initial/target.json'))
    install.recover_enrollment()
    legacy_restored()

    # Crash after candidate switch bypasses normal exception recovery. Exercise
    # the same CLI entry point the timer invokes after reboot.
    switch = u.switch
    def crashed_switch(path):
        switch(path)
        if Path(path).name == 'good':raise KeyboardInterrupt('power loss')
    with patch.object(u,'switch',crashed_switch):
        try:install.install(3)
        except KeyboardInterrupt:pass
        else:raise AssertionError('interruption expected')
    assert (u.ROOT/'enrollment.json').exists()
    u.run('systemctl','stop','pir-updater.timer')
    fcntl.flock(lock, fcntl.LOCK_UN)
    import subprocess
    result = subprocess.run([sys.executable, '/opt/pir-updater/pir_updater.py', 'once'], capture_output=True)
    fcntl.flock(lock, fcntl.LOCK_EX)
    assert result.returncode != 0
    assert b'Interrupted enrollment restored' in result.stderr, result.stderr
    legacy_restored()

    # Retry installs v2 using the separately bootstrapped verifier v-bootstrap.
    attempt()
    u.run('systemctl','stop','pir-updater.timer')
    r=u.Reconciler()
    assert r.matches(cfg)
    assert r.status['converged']
    assert (u.ROOT/'current').resolve().name == 'good'
    assert b'v-bootstrap' in (u.ROOT/'verifier').read_bytes()
    # Installer rerun is a no-op for the current target and preserves trust.
    pid=u.run('systemctl','show',u.NAME,'--property=MainPID','--value')
    verifier=(u.ROOT/'verifier').read_bytes()
    install.install(3)
    u.run('systemctl','stop','pir-updater.timer')
    assert u.run('systemctl','show',u.NAME,'--property=MainPID','--value') == pid
    assert (u.ROOT/'verifier').read_bytes() == verifier

# Normal post-enrollment rollback retains strict metadata checking.
bad=stage(r,'bad',{'config':{'binary_tag':'v3','snapshot_height':3484460}})
(bad/'fail').touch()
try:r.activate(bad)
except RuntimeError:pass
else:raise AssertionError('failure expected')
assert r.matches(cfg)
assert not (u.ROOT/'transaction.json').exists()
print('PASS: legacy bootstrap, authentication/staging failure, timer failure, legacy rollback, crash recovery, rerun, and managed rollback')
