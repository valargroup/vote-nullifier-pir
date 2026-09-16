import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import pir_apm_updater as u


class ApmUpdaterTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for name, value in [('ROOT', self.root), ('SETTINGS', self.root / 'settings.json'),
                            ('STATE', self.root / 'state.json'), ('BINARY', self.root / 'pir-apm'),
                            ('SERVICE', self.root / 'unit'), ('DEFAULTS', self.root / 'defaults')]:
            p = patch.object(u, name, value); p.start(); self.addCleanup(p.stop)
        u.save(u.SETTINGS, {'config_url': 'https://example.com/stage/pir.json'})
        self.u = u.Updater()

    def config(self, body):
        return patch.object(u, 'fetch', return_value=json.dumps(body).encode())

    def test_a_malformed_tag_is_never_interpolated_into_a_url(self):
        # binary_tag is unauthenticated, so a crafted value must not steer the
        # download anywhere.
        for tag in ('../../evil', 'v1.0.0/../x', 'https://evil.test/x', 'latest', '', 'v1.0.0 ;rm'):
            with self.subTest(tag=tag), self.config({'binary_tag': tag}):
                with self.assertRaisesRegex(RuntimeError, 'malformed binary_tag'):
                    self.u.desired_tag()

    def test_accepts_stable_and_release_candidate_tags(self):
        for tag in ('v0.12.0', 'v0.12.0-rc.5', 'v1.2.3'):
            with self.subTest(tag=tag), self.config({'binary_tag': tag}):
                self.assertEqual(self.u.desired_tag(), tag)

    def test_a_config_without_a_binary_tag_leaves_the_sidecar_alone(self):
        with self.config({'snapshot_height': 4245460}):
            self.assertIsNone(self.u.desired_tag())
        with self.config({'snapshot_height': 4245460}), patch.object(self.u, 'stage') as stage:
            self.u.once()
            stage.assert_not_called()

    def test_an_unlisted_asset_is_never_installed(self):
        # Fail closed rather than install something SHA256SUMS does not cover.
        sums = b'%s  nf-server-linux-amd64\n' % (b'a' * 64)
        with patch.object(u, 'fetch', return_value=sums), patch.object(u, 'download') as download:
            with self.assertRaisesRegex(RuntimeError, 'absent from SHA256SUMS'):
                self.u.stage('v0.12.0')
            download.assert_not_called()

    def test_a_matching_tag_does_no_work(self):
        u.BINARY.write_bytes(b'installed')
        self.u.state['installed_tag'] = 'v0.12.0'
        with self.config({'binary_tag': 'v0.12.0'}), patch.object(self.u, 'stage') as stage:
            self.u.once()
            stage.assert_not_called()

    def test_a_missing_binary_reinstalls_even_at_the_same_tag(self):
        self.u.state['installed_tag'] = 'v0.12.0'
        with self.config({'binary_tag': 'v0.12.0'}), \
                patch.object(self.u, 'stage', side_effect=RuntimeError('stage reached')):
            with self.assertRaisesRegex(RuntimeError, 'stage reached'):
                self.u.once()

    def test_an_older_tag_is_followed_so_signed_rollbacks_carry_the_sidecar(self):
        u.BINARY.write_bytes(b'installed')
        self.u.state['installed_tag'] = 'v0.12.0'
        with self.config({'binary_tag': 'v0.11.2'}), \
                patch.object(self.u, 'stage', side_effect=RuntimeError('stage reached')):
            with self.assertRaisesRegex(RuntimeError, 'stage reached'):
                self.u.once()

    def test_a_sidecar_that_fails_health_is_rolled_back(self):
        u.BINARY.write_bytes(b'old apm')
        u.SERVICE.write_bytes(b'old unit')
        staged = self.root / 'staged'
        staged.mkdir()
        (staged / u.ASSET).write_bytes(b'new apm')
        (staged / u.UNIT_ASSET).write_bytes(b'new unit')
        with patch.object(u, 'run'), patch.object(self.u, 'healthy', return_value=False):
            with self.assertRaisesRegex(RuntimeError, 'did not become healthy'):
                self.u.activate('v0.12.0', staged / u.ASSET, staged / u.UNIT_ASSET)
        self.assertEqual(u.BINARY.read_bytes(), b'old apm')
        self.assertEqual(u.SERVICE.read_bytes(), b'old unit')
        self.assertNotEqual(self.u.state.get('installed_tag'), 'v0.12.0')

    def test_a_healthy_activation_records_the_tag(self):
        u.BINARY.write_bytes(b'old apm')
        u.SERVICE.write_bytes(b'old unit')
        staged = self.root / 'staged'
        staged.mkdir()
        (staged / u.ASSET).write_bytes(b'new apm')
        (staged / u.UNIT_ASSET).write_bytes(b'new unit')
        with patch.object(u, 'run'), patch.object(self.u, 'healthy', return_value=True):
            self.u.activate('v0.12.0', staged / u.ASSET, staged / u.UNIT_ASSET)
        self.assertEqual(u.BINARY.read_bytes(), b'new apm')
        self.assertEqual(u.SERVICE.read_bytes(), b'new unit')
        self.assertEqual(self.u.state['installed_tag'], 'v0.12.0')
        self.assertIsNone(self.u.state['error'])

    def test_requests_identify_the_updater_and_require_https(self):
        with self.assertRaises(ValueError):
            u.fetch('http://example.com/pir.json')
        with self.assertRaises(ValueError):
            u.download('http://example.com/a', self.root / 'a', '0' * 64)

    def test_sums_parsing_ignores_noise(self):
        text = ('%s  pir-apm-linux-amd64\n' % ('a' * 64)) + 'garbage line\n' + \
               ('%s *pir-apm.service\n' % ('b' * 64))
        sums = u.parse_sums(text)
        self.assertEqual(sums['pir-apm-linux-amd64'], 'a' * 64)
        self.assertEqual(sums['pir-apm.service'], 'b' * 64)
        self.assertNotIn('garbage', sums)

    def test_listen_address_is_read_not_written(self):
        u.DEFAULTS.write_bytes(b'PIR_APM_LISTEN=127.0.0.1:3099\nPIR_APM_SLACK_WEBHOOK_URL=secret\n')
        before = u.DEFAULTS.read_bytes()
        self.assertEqual(u.listen_address(), ('127.0.0.1', 3099))
        self.assertEqual(u.DEFAULTS.read_bytes(), before)

    def test_listen_address_falls_back_without_a_defaults_file(self):
        self.assertEqual(u.listen_address(), ('127.0.0.1', 3002))

    def test_never_touches_the_serving_updater(self):
        # Decoupling is the point of this service, so pin it against the
        # serving updater's paths, lock and unit. Prose may still name them.
        source = Path(__file__).with_name('pir_apm_updater.py').read_text()
        code = '\n'.join(line.split('#', 1)[0] for line in source.split('"""', 2)[2].splitlines())
        for forbidden in ('/opt/pir-updater', '/var/lib/pir-updater', 'pir-update.lock',
                          'nullifier-query-server', '/opt/nf-ingest/nf-server'):
            self.assertNotIn(forbidden, code)


if __name__ == '__main__':
    unittest.main()
