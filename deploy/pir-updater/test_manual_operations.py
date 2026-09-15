"""Execute checked-in entrypoints against isolated paths; never contact a host."""
import fcntl
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import textwrap
import unittest

REPO = Path(__file__).resolve().parents[2]


def scripts(path):
    """Read literal SSH script blocks without requiring a YAML package on hosts."""
    lines = path.read_text().splitlines()
    blocks = []
    for index, line in enumerate(lines):
        if line.strip() != 'script: |':
            continue
        indent = len(line) - len(line.lstrip())
        body = []
        for following in lines[index + 1:]:
            if following.strip() and len(following) - len(following.lstrip()) <= indent:
                break
            body.append(following)
        blocks.append(textwrap.dedent('\n'.join(body)))
    return blocks


@unittest.skipUnless(shutil.which('flock'), 'Linux util-linux required')
class ManualOperationsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.marker = self.root / 'executed'
        self.lock = self.root / 'run/lock/pir-update.lock'
        self.lock.parent.mkdir(parents=True)
        self.state = self.root / 'opt/pir-updater'
        self.state.mkdir(parents=True)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.write_executable(self.bin / 'sudo', '#!/bin/bash\nif [[ ${1:-} == --preserve-env=* ]]; then shift; fi\nexec "$@"\n')
        self.write_executable(self.bin / 'uname', '#!/bin/bash\nif [[ $1 == -s ]]; then echo Linux; else echo x86_64; fi\n')
        self.write_executable(self.bin / 'dpkg-query', '#!/bin/bash\necho "install ok installed"\n')
        # The first would-be external operation stops execution, checking that
        # the parent still owns the lock before any live files could be changed.
        probe = ('#!/bin/bash\n'
                 f'if flock -n "{self.lock}" -c true; then echo unlocked > "{self.marker}"; '
                 f'else echo locked > "{self.marker}"; fi\n'
                 'if [[ ${2:-} == --help ]]; then echo --zcash-network; exit 0; fi\n'
                 'exit 79\n')
        for name in ('curl', 'systemctl', 'install', 'apt-get'):
            self.write_executable(self.bin / name, probe)
        self.write_executable(self.root / 'opt/nf-ingest/nf-server', probe)
        self.write_executable(self.root / 'tmp/pir-deploy-fixture-fixture-fixture/nf-server', probe)
        defaults = self.root / 'etc/default/nf-server'
        defaults.parent.mkdir(parents=True)
        defaults.write_text('SVOTE_ZCASH_NETWORK=main\n')
        self.env = dict(os.environ, PATH=f'{self.bin}:{os.environ["PATH"]}', SVOTE_ZCASH_NETWORK='main')
        deploy = scripts(REPO / '.github/actions/deploy-pir-host/action.yml')
        self.assertEqual(len(deploy), 2)
        restart = scripts(REPO / '.github/workflows/restart.yml')
        self.assertEqual(len(restart), 2)
        self.entrypoints = dict(zip(('preflight', 'deploy', 'restart-backup', 'restart-primary'), deploy + restart))
        for name in ('start', 'update'):
            self.entrypoints[name] = (REPO / f'scripts/{name}_pir.sh.template').read_text()

    @staticmethod
    def write_executable(path, text):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        path.chmod(0o755)

    def execute(self, name):
        code = self.entrypoints[name]
        for prefix in ('/run/lock/', '/opt/', '/etc/', '/tmp/pir-deploy-', '/usr/local/bin/'):
            code = code.replace(prefix, str(self.root) + prefix)
        code = re.sub(r'\$\{\{.*?\}\}', 'fixture', code)
        code = code.replace('${EUID:-0}', '0').replace('__RELEASE_TAG__', 'v1.2.3')
        return subprocess.run(['bash', '-c', code], env=self.env, capture_output=True, text=True, timeout=10)

    def test_enrolled_and_interrupted_hosts_rejected_before_any_operation(self):
        for state in ('settings.json', 'enrollment.json'):
            (self.state / state).write_text('{}')
            for name in self.entrypoints:
                with self.subTest(state=state, entrypoint=name):
                    result = self.execute(name)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('enrolled in signed updates', result.stderr)
                    self.assertFalse(self.marker.exists())
            (self.state / state).unlink()

    def test_busy_lock_rejected_before_any_operation(self):
        with self.lock.open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            for name in self.entrypoints:
                with self.subTest(entrypoint=name):
                    result = self.execute(name)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('Another PIR', result.stderr)
                    self.assertFalse(self.marker.exists())

    def test_unmanaged_operations_reach_external_commands_with_lock_held(self):
        self.assertEqual(self.execute('preflight').returncode, 0)
        for name in self.entrypoints.keys() - {'preflight'}:
            with self.subTest(entrypoint=name):
                result = self.execute(name)
                self.assertNotEqual(result.returncode, 0)  # deliberately stopped by probe
                self.assertTrue(self.marker.exists(), result.stderr)
                self.assertEqual(self.marker.read_text().strip(), 'locked')
                self.marker.unlink()

    def test_enrollment_between_preflight_and_install_is_rejected(self):
        self.assertEqual(self.execute('preflight').returncode, 0)
        (self.state / 'settings.json').write_text('{}')
        result = self.execute('deploy')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('enrolled in signed updates', result.stderr)
        self.assertFalse(self.marker.exists())
        action = (REPO / '.github/actions/deploy-pir-host/action.yml').read_text()
        self.assertIn('target: /tmp/pir-deploy-', action)
        self.assertNotIn('target: /opt/nf-ingest', action)


if __name__ == '__main__':
    unittest.main()
