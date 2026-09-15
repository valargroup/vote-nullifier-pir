#!/usr/bin/python3
"""Runs only inside the disposable systemd container built from tests/Dockerfile."""
import json
import os
from pathlib import Path
import shutil
import sys
sys.path.insert(0,'/opt/pir-updater')
import pir_updater as u
import install
assert Path('/run/systemd/system').is_dir()
Path('/opt/nf-ingest/pir-data/main').mkdir(parents=True)
u.atomic(Path('/opt/nf-ingest/pir-data/main/pir_root.json'),b'{}')
u.atomic(Path('/opt/nf-ingest/nf-server'),Path('/opt/pir-updater/tests/fake_server.py').read_bytes(),0o755)
u.save(Path('/opt/nf-ingest/target.json'),{'binary_tag':'v1','snapshot_height':3484440})
u.atomic(Path('/etc/default/nf-server'),b'SVOTE_ZCASH_NETWORK=main\nSVOTE_PIR_CONFIG_URL=https://example.com/prod/pir.json\nSVOTE_PIR_DATA_DIR=/opt/nf-ingest/pir-data/main\n')
unit=b'[Unit]\nDescription=PIR fixture\n[Service]\nExecStart=/opt/nf-ingest/nf-server serve --port 3000\nRestart=no\n[Install]\nWantedBy=multi-user.target\n'
u.atomic(u.SERVICE,unit)
u.run('systemctl','daemon-reload');u.run('systemctl','start',u.NAME)
import time
for _ in range(30):
    try:
        import urllib.request
        urllib.request.urlopen('http://127.0.0.1:3000/ready').close();break
    except Exception:time.sleep(.1)
# Fault immediately after creating the temporary managed link. Recovery must
# remove that link, restore the original executable, and allow enrollment retry.
from unittest.mock import patch
replace = os.replace
def interrupt_replace(source, destination):
    if str(source).endswith('nf-server.managed'):
        raise RuntimeError('simulated enrollment interruption')
    return replace(source, destination)
with patch.object(install.os, 'replace', side_effect=interrupt_replace):
    try:
        install.install(3)
    except RuntimeError:
        pass
    else:
        raise AssertionError('interruption not reached')
assert Path('/opt/nf-ingest/nf-server.managed').is_symlink()
install.recover_enrollment()
assert not Path('/opt/nf-ingest/nf-server.managed').is_symlink()
assert not Path('/opt/nf-ingest/nf-server').is_symlink()
# Failure after settings are durable must not make a retry report false success.
real_run = install.run
def interrupt_enable(*args, **kwargs):
    if args[:2] == ('systemctl', 'enable'):
        raise RuntimeError('simulated timer enable failure')
    return real_run(*args, **kwargs)
with patch.object(install, 'run', side_effect=interrupt_enable):
    try:
        install.install(3)
    except RuntimeError:
        pass
    else:
        raise AssertionError('enable interruption not reached')
assert (u.ROOT / 'settings.json').exists()
install.recover_enrollment()
assert not (u.ROOT / 'settings.json').exists()
install.install(3)
u.run('systemctl','stop','pir-updater.timer')
r=u.Reconciler()
initial=(u.ROOT/'current').resolve()
# Seed retained initial fixture target; the installer created this record itself.
assert (initial/'target.json').exists()
for name,tag,height,fail in [('good','v2',3484450,False),('bad','v3',3484460,True)]:
    path=u.ROOT/'generations'/name;path.mkdir();(path/'data').mkdir()
    u.atomic(path/'nf-server',Path('/opt/pir-updater/tests/fake_server.py').read_bytes(),0o755)
    u.atomic(path/'service',unit);u.save(path/'target.json',{'binary_tag':tag,'snapshot_height':height})
    if fail:(path/'fail').touch()
    try:r.activate(path)
    except RuntimeError:
        assert fail
    else:assert not fail
assert (u.ROOT/'current').resolve().name=='good'
assert r.matches({'binary_tag':'v2','snapshot_height':3484450})
assert not (u.ROOT/'transaction.json').exists()
assert r.status['rollbacks']==1
# Simulate power loss after current pointer switched but before successful activation.
bad=u.ROOT/'generations'/'bad'; good=u.ROOT/'generations'/'good'
u.save(u.ROOT/'transaction.json',{'previous':str(good),'target':str(bad),'previous_unit':u.base64.b64encode(unit).decode(),'previous_target':u.read(good/'target.json')})
u.run('systemctl','stop',u.NAME);u.switch(bad)
r.recover()
assert r.matches({'binary_tag':'v2','snapshot_height':3484450})
print('PASS: real systemd enrollment, activation, failed activation rollback, and interrupted activation recovery')
