import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("bundle", TOOLS / "build-docs-bundle.py")
bundle = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(bundle)


class BundleTests(unittest.TestCase):
    def test_fixture_roundtrip_and_reproducibility(self):
        fixtures = TOOLS.parent / "crates/agentgateway/src/ui/testdata"
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "docs.pack"
            bundle.build(fixtures / "site", output, "test-v1")
            data = output.read_bytes()
            # Compare decoded content rather than compressor-version-specific bytes.
            for packed in [data, (fixtures / "docs.pack").read_bytes()]:
                self.assertEqual(packed[:8], b"AGWDOCS1")
                length, = struct.unpack("<Q", packed[8:16])
                manifest = json.loads(packed[16:16 + length])
                self.assertEqual(manifest["version"], "test-v1")
                for asset in manifest["assets"]:
                    offset = 16 + length + asset["offset"]
                    content = packed[offset:offset + asset["length"]]
                    self.assertEqual(hashlib.sha256(content).hexdigest(), asset["sha256"])
                    if asset["encoding"] == "gzip":
                        content = gzip.decompress(content)
                    self.assertEqual(len(content), asset["originalLength"])
                    self.assertEqual(content, (fixtures / "site" / asset["path"]).read_bytes())
            bundle.build(fixtures / "site", output, "test-v1")
            self.assertEqual(data, output.read_bytes())
            bundle.build(fixtures / "site", output, "test-v2", "guide/")
            self.assertNotEqual(data, output.read_bytes())

    def test_invalid_inputs_preserve_existing_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "site"
            source.mkdir()
            (source / "index.html").write_text("hello")
            output = root / "docs.pack"
            output.write_bytes(b"previous version")
            for entry in ["../index.html", "/index.html", "missing.html", "x?y", "x%20y"]:
                with self.assertRaises(ValueError):
                    bundle.build(source, output, "v1", entry)
                self.assertEqual(output.read_bytes(), b"previous version")
            with self.assertRaises(ValueError):
                bundle.build(source, source / "docs.pack", "v1")
            (source / "link").symlink_to(output)
            with self.assertRaises(ValueError):
                bundle.build(source, output, "v1")
            self.assertEqual(output.read_bytes(), b"previous version")


if __name__ == "__main__":
    unittest.main()
