import contextlib
import copy
import io
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import verify_round_imt as verifier


class VerifyRoundIMTTests(unittest.TestCase):
    def setUp(self):
        self.args = verifier.parser().parse_args([
            "--vote-node", "http://localhost:26657", "--vote-chain-id", "vote-test",
            "--zcash-network", "main", "--trusted-block-hash", "22" * 32,
            "--block-rpc-url", "http://localhost:8232", "--mode", "raw-blocks",
        ])
        self.round = dict(chain_id="vote-test", round_id="11" * 32, computed_round_id="11" * 32,
                          round_id_matches=True, created_at_height=123, snapshot_height=3428150,
                          snapshot_blockhash="22" * 32, nullifier_imt_root="33" * 32,
                          proposals_hash="44" * 32, nc_root="55" * 32, vote_end_time=2000000000,
                          status="SESSION_STATUS_PENDING")
        self.rebuild = dict(zcash_network="main", nullifier_pool="ironwood", dataset_version=2,
                            height=3428150, trusted_block_hash="22" * 32, expected_circuit_root="33" * 32,
                            computed_circuit_root="33" * 32, verified_blocks=8, matches=True)
        self.commands = []

    def invoke(self, command, progress=False):
        self.commands.append(command)
        if "verify-round" in command:
            return 0, copy.deepcopy(self.round)
        if "build-info" in command:
            return 0, {"commit_sha": "test-revision"}
        if "verify-root" in command:
            self.assertEqual(command[command.index("--height") + 1], "3428150")
            self.assertEqual(command[command.index("--expected-circuit-root") + 1], "33" * 32)
            return (0 if self.rebuild["matches"] else 1), copy.deepcopy(self.rebuild)
        self.fail(f"unexpected command {command}")

    def verify(self, invoke=None):
        with patch.object(verifier, "invoke", side_effect=invoke or self.invoke), \
             patch.object(verifier, "fingerprint", return_value="test-sha"), \
             contextlib.redirect_stderr(io.StringIO()):
            return verifier.verify(self.args)

    def test_latest_pending_round_succeeds_without_ea_key(self):
        result = self.verify()
        self.assertEqual(result["outcome"], "verified")
        self.assertIn("--latest", self.commands[0])
        queries = [c for c in self.commands if "verify-round" in c]
        self.assertIn(self.round["round_id"], queries[1])
        self.assertIn("--latest", queries[2])

    def test_explicit_round_never_selects_latest(self):
        self.args.round_id = self.round["round_id"]
        self.assertEqual(self.verify()["outcome"], "verified")
        self.assertFalse(any("--latest" in c for c in self.commands))

    def test_wrong_independent_anchor_does_not_rebuild(self):
        self.args.trusted_block_hash = "66" * 32
        self.assertEqual(self.verify()["outcome"], "mismatch")
        self.assertEqual(len(self.commands), 1)

    def test_root_mismatch_prints_both_roots(self):
        self.rebuild.update(matches=False, computed_circuit_root="66" * 32)
        result = self.verify()
        self.assertEqual(result["outcome"], "mismatch")
        self.assertEqual(result["rebuild"]["computed_circuit_root"], "66" * 32)

    def test_bad_exit_or_echoed_snapshot_cannot_pass(self):
        for field, bad in (("matches", False), ("height", 3428160), ("zcash_network", "test"), ("verified_blocks", 0)):
            with self.subTest(field=field):
                saved = copy.deepcopy(self.rebuild)
                self.rebuild[field] = bad
                with self.assertRaises(verifier.VerificationError):
                    self.verify()
                self.rebuild = saved

    def test_unavailable_history_cannot_pass(self):
        def run(command, progress=False):
            if "verify-root" in command:
                raise verifier.VerificationError("history unavailable")
            return self.invoke(command, progress)
        with self.assertRaisesRegex(verifier.VerificationError, "history unavailable"):
            self.verify(run)

    def test_wrong_round_id_cannot_pass(self):
        self.round["round_id_matches"] = False
        with self.assertRaisesRegex(verifier.VerificationError, "binding"):
            self.verify()

    def test_changed_round_and_newer_round_are_distinct(self):
        queries = 0
        def run(command, progress=False):
            nonlocal queries
            code, result = self.invoke(command, progress)
            if "verify-round" in command:
                queries += 1
                if queries == 2:
                    result["snapshot_height"] += 10
            return code, result
        self.assertEqual(self.verify(run)["outcome"], "incomplete")
        queries = 0
        def newer(command, progress=False):
            nonlocal queries
            code, result = self.invoke(command, progress)
            if "verify-round" in command:
                queries += 1
                if queries == 3:
                    result["round_id"] = result["computed_round_id"] = "77" * 32
            return code, result
        result = self.verify(newer)
        self.assertEqual(result["outcome"], "verified")
        self.assertEqual(result["round"]["round_id"], "11" * 32)
        self.assertIn("77" * 32, result["notice"])

    def test_inspection_needs_no_raw_node_or_anchor(self):
        self.args.inspect = True
        self.args.trusted_block_hash = self.args.block_rpc_url = None
        self.assertEqual(self.verify()["outcome"], "inspection")
        self.assertEqual(len(self.commands), 1)

    def use_pir_sync(self):
        self.args = verifier.parser().parse_args([
            "--vote-node", "http://localhost:26657", "--vote-chain-id", "vote-test",
            "--zcash-network", "main", "--lwd-url", "https://lwd.example:443",
        ])
        self.metadata = dict(zcash_network="main", nullifier_pool="ironwood", dataset_version=2,
                             height=3428150, circuit_root="33" * 32, pir_root="88" * 32)

    def sync(self, command, **kwargs):
        self.commands.append(command)
        self.assertEqual(command[1], "sync")
        self.assertEqual(command[command.index("--max-height") + 1], "3428150")
        self.assertEqual(command[command.index("--voting-config-url") + 1], "")
        self.assertEqual(command[command.index("--lwd-url") + 1], self.args.lwd_url)
        self.assertEqual(kwargs["env"]["LWD_URLS"], self.args.lwd_url)
        self.assertNotIn("SVOTE_PIR_SYNC_RESET", kwargs["env"])
        self.sync_dir = Path(command[command.index("--pir-data-dir") + 1])
        self.assertEqual(list(self.sync_dir.iterdir()), [])
        self.assertEqual(command[command.index("--output-dir") + 1], str(self.sync_dir))
        (self.sync_dir / "pir_root.json").write_text(json.dumps(self.metadata))
        return subprocess.CompletedProcess(command, 0)

    def test_default_rebuild_uses_fresh_pir_sync_and_explicit_source(self):
        self.use_pir_sync()
        with patch.dict(verifier.os.environ, {"LWD_URLS": "https://wrong.example", "SVOTE_PIR_SYNC_RESET": "1"}), \
             patch.object(verifier.subprocess, "run", side_effect=self.sync):
            result = self.verify()
        self.assertEqual(result["outcome"], "verified")
        self.assertEqual(result["method"], "pir-sync")
        self.assertTrue(result["rebuild"]["matches"])
        self.assertNotIn("verified_blocks", result["rebuild"])
        self.assertFalse(self.sync_dir.exists())
        self.assertFalse(any("verify-root" in c for c in self.commands))
        self.assertEqual(len([c for c in self.commands if "verify-round" in c]), 3)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            verifier.print_result(result)
        self.assertIn("Root comparison: MATCH", output.getvalue())
        self.assertIn("Does not authenticate raw blocks", output.getvalue())

    def test_pir_sync_wrong_height_network_or_dataset_cannot_pass(self):
        for key, value in (("height", 3428140), ("height", 3428160), ("zcash_network", "test"),
                           ("nullifier_pool", "orchard"), ("dataset_version", 1), ("circuit_root", "bad")):
            with self.subTest(key=key, value=value):
                self.use_pir_sync()
                self.metadata[key] = value
                with patch.object(verifier.subprocess, "run", side_effect=self.sync), \
                     self.assertRaises(verifier.VerificationError):
                    self.verify()
                self.assertFalse(self.sync_dir.exists())

    def test_pir_sync_root_mismatch_prints_both_roots(self):
        self.use_pir_sync()
        self.metadata["circuit_root"] = "66" * 32
        with patch.object(verifier.subprocess, "run", side_effect=self.sync):
            result = self.verify()
        self.assertEqual(result["outcome"], "mismatch")
        self.assertEqual(result["round"]["nullifier_imt_root"], "33" * 32)
        self.assertEqual(result["rebuild"]["computed_circuit_root"], "66" * 32)

    def test_pir_sync_failed_process_cannot_pass_even_with_matching_metadata(self):
        self.use_pir_sync()
        def failed(command, **kwargs):
            self.sync(command, **kwargs)
            return subprocess.CompletedProcess(command, 1)
        with patch.object(verifier.subprocess, "run", side_effect=failed), \
             self.assertRaisesRegex(verifier.VerificationError, "did not complete"):
            self.verify()
        self.assertFalse(self.sync_dir.exists())

    def test_pir_sync_missing_metadata_cannot_pass(self):
        self.use_pir_sync()
        with patch.object(verifier.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)), \
             self.assertRaisesRegex(verifier.VerificationError, "root metadata"):
            self.verify()

    def test_pir_sync_needs_explicit_source_and_rejects_raw_options(self):
        self.use_pir_sync()
        self.args.lwd_url = None
        with self.assertRaisesRegex(verifier.VerificationError, "explicit --lwd-url"):
            self.verify()
        self.args.trusted_block_hash = "22" * 32
        with self.assertRaisesRegex(verifier.VerificationError, "--mode raw-blocks"):
            self.verify()

    def test_pir_sync_changed_round_does_not_pass(self):
        self.use_pir_sync()
        queries = 0
        def changed(command, progress=False):
            nonlocal queries
            code, result = self.invoke(command, progress)
            if "verify-round" in command:
                queries += 1
                if queries == 2:
                    result["snapshot_blockhash"] = "77" * 32
            return code, result
        with patch.object(verifier.subprocess, "run", side_effect=self.sync):
            self.assertEqual(self.verify(changed)["outcome"], "incomplete")


if __name__ == "__main__":
    unittest.main()
