"""Approved logo asset and native-binding contracts. Requires Pillow."""
from pathlib import Path
from hashlib import sha256
from io import BytesIO
import json
import struct
import unittest
from PIL import Image
ROOT = Path(__file__).resolve().parents[1]
APP = "AnimateStrike"
PACKAGE = "animatestrike"
APPROVED = "423c61242f423dc88d905eed52cbb5c05536eaa03e0fafa972423a614c003a9e"
class BrandingContract(unittest.TestCase):
    def test_exact_approved_source_and_provenance(self):
        path = ROOT / f"assets/branding/{APP}.png"
        self.assertEqual(sha256(path.read_bytes()).hexdigest(), APPROVED)
        self.assertEqual(Image.open(BytesIO(path.read_bytes())).size, (512, 512))
        metadata = json.loads((path.parent / "provenance.json").read_text(encoding="utf-8"))
        self.assertEqual(metadata["sha256"], APPROVED)
    def test_runtime_derivative_and_binding(self):
        source = Image.open(ROOT / f"assets/branding/{APP}.png").convert("RGBA")
        self.assertTrue((ROOT / f"assets/branding/{APP}-128.rgba").exists(), "runtime derivative missing")
        self.assertEqual((ROOT / f"assets/branding/{APP}-128.rgba").read_bytes(), source.resize((128, 128), Image.Resampling.LANCZOS).tobytes())
        code = (ROOT / f"apps/{PACKAGE}/src/main.rs").read_text(encoding="utf-8")
        self.assertIn(".with_icon(app_icon())", code)
        self.assertIn(f'include_bytes!("../../../assets/branding/{APP}-128.rgba")', code)
        self.assertIn("width: 128, height: 128", code)
    def test_readme_uses_canonical_source(self):
        original = ROOT / f"docs/branding/{APP}.webp"
        self.assertIn(f'docs/branding/{APP}.webp', (ROOT / "README.md").read_text(encoding="utf-8"))
        self.assertEqual(sha256(original.read_bytes()).hexdigest(), "24154c71c4aa59e555602981cfd7482e1646ca2e9d2572aa2cc94e3d809a102e")
        with Image.open(original) as approved, Image.open(ROOT / f"assets/branding/{APP}.png") as runtime:
            self.assertEqual(approved.convert("RGBA").tobytes(), runtime.convert("RGBA").tobytes())
    def test_windows_resource(self):
        build_script = ROOT / f"apps/{PACKAGE}/build.rs"
        self.assertTrue(build_script.exists(), "Windows resource binding missing")
        code = build_script.read_text(encoding="utf-8")
        self.assertIn(f'set_icon("../../assets/branding/{APP}.ico")', code)
        self.assertIn(f'cargo:rerun-if-changed=../../assets/branding/{APP}.ico', code)
        source = Image.open(ROOT / f"assets/branding/{APP}.png").convert("RGBA")
        data = (ROOT / f"assets/branding/{APP}.ico").read_bytes()
        sizes = (16, 24, 32, 48, 64, 128, 256)
        self.assertEqual(struct.unpack_from("<HHH", data), (0, 1, len(sizes)))
        for i, size in enumerate(sizes):
            w,h,_,_,_,_,length,offset = struct.unpack_from("<BBBBHHII", data, 6+16*i)
            self.assertEqual((w or 256,h or 256),(size,size))
            frame = Image.open(BytesIO(data[offset:offset+length])).convert("RGBA")
            self.assertEqual(frame.tobytes(),source.resize((size,size),Image.Resampling.LANCZOS).tobytes())
if __name__ == "__main__":
    unittest.main()
