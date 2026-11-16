"""Exercise the exact metadata validator run on the snapshot publisher."""

import copy
import json
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/publish-snapshot.yml"


class SnapshotValidationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        workflow = WORKFLOW.read_text()
        start = workflow.index("            validate_snapshot_root() {")
        end = workflow.index("            # End snapshot root validation.", start)
        cls.validator = textwrap.dedent(workflow[start:end])

    def metadata(self, depth):
        rows = 1 << 12
        return {
            "zcash_network": "main",
            "nullifier_pool": "ironwood",
            "dataset_version": 2,
            "height": 3578540,
            "num_ranges": 1 << depth,
            "pir_depth": depth,
            "pir_layout": {
                "pir_depth": depth,
                "tier0_layers": 12,
                "tier1_layers": depth - 12,
                "poly_len": 4096,
            },
            "tier1_rows": rows,
            "tier1_row_bytes": (1 << (depth - 12)) * 96,
            "tier0_bytes": rows * 96 - 32,
        }

    def valid(self, metadata):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "pir_root.json"
            path.write_text(json.dumps(metadata))
            script = (
                "set -euo pipefail\n"
                "SVOTE_ZCASH_NETWORK=main\n"
                "EXPECTED_NULLIFIER_POOL=ironwood\n"
                "EXPECTED_DATASET_VERSION=2\n"
                + self.validator
                + '\nvalidate_snapshot_root "$1"\n'
            )
            result = subprocess.run(
                ["bash", "-c", script, "snapshot-validator", str(path)],
                capture_output=True,
                text=True,
                timeout=5,
            )
            return result.returncode == 0

    def test_existing_and_expanded_layouts(self):
        for depth in [19, 20]:
            with self.subTest(depth=depth):
                self.assertTrue(self.valid(self.metadata(depth)))
        snapshot = self.metadata(20)
        snapshot["num_ranges"] = 619172
        self.assertTrue(self.valid(snapshot))

    def test_reject_inconsistent_identity_dimensions_and_capacity(self):
        original = self.metadata(20)
        for key, value in {
            "zcash_network": "test",
            "nullifier_pool": "orchard",
            "dataset_version": 1,
            "pir_depth": 19,
            "tier1_rows": 8192,
            "tier1_row_bytes": 6144,
            "tier0_bytes": 786400,
            "num_ranges": (1 << 20) + 1,
            "height": None,
        }.items():
            with self.subTest(key=key):
                metadata = copy.deepcopy(original)
                metadata[key] = value
                self.assertFalse(self.valid(metadata))
        for depth in [18, 21, 23, 24]:
            self.assertFalse(self.valid(self.metadata(depth)))
        for key, value in {"poly_len": 2048, "tier1_layers": 9, "tier0_layers": 13.5}.items():
            metadata = copy.deepcopy(original)
            metadata["pir_layout"][key] = value
            self.assertFalse(self.valid(metadata))


if __name__ == "__main__":
    unittest.main()
