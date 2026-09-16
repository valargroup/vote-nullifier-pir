import base64
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, Mock
import pir_updater as u


class UpdateTests(unittest.TestCase):
    def test_legacy_recovery_disables_supported_discovery_options(self):
        for modern in (False, True):
            help_text = b'--pir-data-dir --voting-config-url'
            if modern: help_text += b' --pir-config-url'
            with patch.object(u, 'run', return_value=help_text):
                override = u.legacy_dropin(Path('/legacy'), '/data/a b%$c')
            self.assertIn(b'--voting-config-url=', override)
            self.assertEqual(b'--pir-config-url=' in override, modern)
            self.assertIn(b'--pir-data-dir "/data/a b%%$$c"', override)

    def test_legacy_recovery_rejects_missing_discovery_control(self):
        for help_text in (b'', b'--pir-data-dir', b'--voting-config-url'):
            with patch.object(u, 'run', return_value=help_text):
                with self.assertRaisesRegex(RuntimeError, 'cannot disable'):
                    u.legacy_dropin(Path('/legacy'), '/data')

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for name, value in [('ROOT', self.root), ('STATUS', self.root / 'status.json'), ('SERVICE', self.root / 'unit'), ('DROPIN', self.root / 'dropin'), ('BINARY', self.root / 'binary')]:
            p = patch.object(u, name, value); p.start(); self.addCleanup(p.stop)
        u.SERVICE.write_bytes(b'previous unit')
        u.save(self.root / 'settings.json', {'scope':'prod', 'network':'main', 'config_url':'https://example.com/prod/pir.json','timeout_secs':1})
        initial = self.root / 'generations' / 'initial'; initial.mkdir(parents=True)
        u.save(initial / 'target.json', {'binary_tag':'v1','snapshot_height':3484440})
        u.switch(initial)
        self.r = u.Reconciler()

    def test_public_requests_identify_the_updater(self):
        # The config gateway rejects Python's default UA with HTTP 403/1010.
        for operation in ('metadata', 'artifact'):
            with self.subTest(operation=operation):
                response = Mock()
                response.url = 'https://example.com/content'
                response.read.side_effect = [b'good', b'']
                response.__enter__ = Mock(return_value=response)
                response.__exit__ = Mock(return_value=False)
                with patch.object(u.urllib.request, 'urlopen', return_value=response) as open_url:
                    if operation == 'metadata':
                        self.assertEqual(u.fetch(response.url), b'good')
                    else:
                        u.download([response.url], self.root / 'artifact', u.hashlib.sha256(b'good').hexdigest())
                    request = open_url.call_args.args[0]
                    self.assertEqual(request.full_url, response.url)
                    self.assertEqual(request.get_header('User-agent'), 'pir-updater/1')

    def test_invalid_signature_never_reaches_stage_or_activation(self):
        with patch.object(u, 'fetch', return_value=b'{}'), patch.object(u, 'run', side_effect=RuntimeError('bad signature')), patch.object(self.r, 'stage') as stage, patch.object(self.r, 'activate') as activate:
            with self.assertRaises(RuntimeError): self.r.once()
            stage.assert_not_called(); activate.assert_not_called()
        self.assertEqual((self.root / 'current').resolve().name, 'initial')

    def test_noop_does_not_download_or_restart(self):
        cfg = {'binary_tag':'v1','snapshot_height':3484440}
        with patch.object(self.r,'target',return_value=('initial',{'config':cfg})), patch.object(self.r,'matches',return_value=True), patch.object(self.r,'stage') as stage, patch.object(u,'run') as run:
            self.r.once(); stage.assert_not_called(); run.assert_not_called()
        self.assertTrue(u.read(u.STATUS)['converged'])

    def test_backoff_and_changed_target(self):
        self.r.status.update(desired_id='new', next_retry=10**12)
        cfg={'config':{'binary_tag':'v2','snapshot_height':3484450}}
        with patch.object(self.r,'target',return_value=('new',cfg)),patch.object(self.r,'stage') as stage:
            self.r.once();stage.assert_not_called()
        with patch.object(self.r,'target',return_value=('different',cfg)),patch.object(self.r,'stage',side_effect=RuntimeError('stage reached')) as stage:
            with self.assertRaises(RuntimeError):self.r.once()
            stage.assert_called_once()

    def candidate(self):
        path=self.root / 'generations' / 'new';path.mkdir();(path/'service').write_bytes(b'new unit')
        u.save(path/'target.json',{'binary_tag':'v2','snapshot_height':3484450});return path

    def test_failed_activation_restores_binary_snapshot_and_unit(self):
        path=self.candidate()
        with patch.object(u,'run'),patch.object(self.r,'wait',side_effect=[RuntimeError('failed candidate'),None]):
            with self.assertRaises(RuntimeError):self.r.activate(path)
        self.assertEqual((self.root/'current').resolve().name,'initial')
        self.assertEqual(u.SERVICE.read_bytes(),b'previous unit')
        self.assertFalse((self.root/'transaction.json').exists())
        self.assertEqual(self.r.status['rollbacks'],1)

    def test_recovery_retains_journal_when_rollback_fails(self):
        path=self.candidate();previous=(self.root/'current').resolve()
        tx={'previous':str(previous),'target':str(path),'previous_unit':base64.b64encode(b'previous unit').decode(),'previous_target':u.read(previous/'target.json')}
        u.save(self.root/'transaction.json',tx);u.switch(path)
        with patch.object(u,'run'),patch.object(self.r,'wait',side_effect=RuntimeError('not ready')):
            with self.assertRaises(RuntimeError):self.r.recover()
        self.assertTrue((self.root/'transaction.json').exists())
        with patch.object(u,'run'),patch.object(self.r,'wait'):
            self.r.recover()
        self.assertFalse((self.root/'transaction.json').exists())

    def test_config_changes_during_staging_do_not_activate(self):
        path=self.candidate();cfg={'config':{'binary_tag':'v2','snapshot_height':3484450}}
        with patch.object(self.r,'target',side_effect=[('new',cfg),('other',cfg)]),patch.object(self.r,'stage',return_value=path),patch.object(self.r,'activate') as activate:
            self.r.once();activate.assert_not_called()
        self.assertFalse(path.exists())

    def test_hash_mismatch_never_leaves_candidate(self):
        response=Mock();response.url='https://example.com/binary';response.read.side_effect=[b'bad',b''];response.__enter__=Mock(return_value=response);response.__exit__=Mock(return_value=False)
        path=self.root/'candidate'
        with patch.object(u.urllib.request,'urlopen',return_value=response):
            with self.assertRaises(RuntimeError):u.download(['https://example.com/binary'],path,'0'*64)
        self.assertFalse(path.exists())

    def test_spaces_failure_uses_verified_fallback(self):
        response=Mock();response.url='https://example.com/binary';response.read.side_effect=[b'good',b''];response.__enter__=Mock(return_value=response);response.__exit__=Mock(return_value=False)
        path=self.root/'candidate'
        with patch.object(u.urllib.request,'urlopen',side_effect=[OSError(),response]):
            u.download(['https://spaces/binary','https://github/binary'],path,u.hashlib.sha256(b'good').hexdigest())
        self.assertEqual(path.read_bytes(),b'good')

    def test_snapshot_manifest_must_authenticate_before_candidate_runs(self):
        self.r.settings.update(snapshot_base='https://example.com')
        with patch.object(u,'fetch',return_value=b'{}'),patch.object(u,'run') as run:
            with self.assertRaises(RuntimeError):self.r.stage('bad',{'config':{'binary_tag':'v2','snapshot_height':3484450},'payload':{'snapshot_manifest_sha256':'0'*64}})
            run.assert_not_called()

    def test_incomplete_enrollment_prevents_updates(self):
        u.save(self.root/'enrollment.json',{})
        with patch.object(self.r,'target') as target:
            with self.assertRaises(RuntimeError):self.r.once()
            target.assert_not_called()


if __name__ == '__main__': unittest.main()
