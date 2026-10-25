"""cog_lint accepts [cog].interfaces, rejects unknown names with the allowed list, and every
cog in this repository declares valid ones."""
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import cog_lint  # noqa: E402


def lint_with(interfaces_line: str | None) -> list[dict]:
    """Copy bme280 into a scratch repo with `interfaces_line` in place of its own, lint it."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp_root = Path(tmp)
        dest = tmp_root / "src" / "cogs" / "bme280"
        shutil.copytree(ROOT / "src" / "cogs" / "bme280", dest, ignore=shutil.ignore_patterns("target"))
        toml = dest / "cog.toml"
        text = re.sub(r"^interfaces = .*\n", "", toml.read_text(), flags=re.M)
        if interfaces_line is not None:
            text = text.replace("[config]\n", interfaces_line + "\n\n[config]\n", 1)
        toml.write_text(text)
        report = cog_lint.run(tmp_root)
        return [f for f in report["findings"] if f["check"] == "L015"]


class InterfaceLint(unittest.TestCase):
    def test_valid_lists_pass(self):
        for line in ('interfaces = ["i2c"]', 'interfaces = ["uart", "gpio"]', 'interfaces = ["none"]'):
            self.assertEqual(lint_with(line), [], line)

    def test_unknown_name_is_rejected_with_the_allowed_list(self):
        (f,) = lint_with('interfaces = ["usb"]')
        self.assertEqual(f["level"], "error")
        self.assertIn("unknown interface 'usb'", f["message"])
        self.assertIn("audio-in", f["message"])
        self.assertIn("none", f["message"])

    def test_repeat_none_with_others_and_empty_are_rejected(self):
        self.assertIn("repeats", lint_with('interfaces = ["i2c", "i2c"]')[0]["message"])
        self.assertIn("stand alone", lint_with('interfaces = ["none", "i2c"]')[0]["message"])
        self.assertIn("empty", lint_with("interfaces = []")[0]["message"])
        self.assertIn("list of strings", lint_with('interfaces = "i2c"')[0]["message"])

    def test_missing_is_a_warning(self):
        (f,) = lint_with(None)
        self.assertEqual(f["level"], "warning")

    def test_every_cog_in_this_repo_declares_valid_interfaces(self):
        report = cog_lint.run(ROOT)
        self.assertEqual([f for f in report["findings"] if f["check"] == "L015"], [])


if __name__ == "__main__":
    unittest.main()
