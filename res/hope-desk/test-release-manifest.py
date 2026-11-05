#!/usr/bin/env python3
"""Tests for release-manifest.py. Run: python3 -I res/hope-desk/test-release-manifest.py"""
import copy
import hashlib
import importlib.util
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("rm", os.path.join(HERE, "release-manifest.py"))
rm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rm)


def fake_inspect(version_for):
    return lambda key, path: (version_for, "1.5.1")


def run(argv):
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        code = rm.main(argv)
    return code, out.getvalue(), err.getvalue()


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.dmg = os.path.join(self.tmp, "Hope-Desk.dmg")
        with open(self.dmg, "wb") as f:
            f.write(b"hello dmg bytes")
        self.sha = hashlib.sha256(b"hello dmg bytes").hexdigest()

    def good(self):
        return rm.build_manifest(57, {"staff": self.dmg}, ["PP-Test-01"], "test", inspect=fake_inspect("hd2.57"))

    def write_file(self, obj, name="latest.json"):
        p = os.path.join(self.tmp, name)
        with open(p, "w") as f:
            f.write(obj if isinstance(obj, str) else json.dumps(obj))
        return p


class WriteTests(Base):
    def test_happy_path(self):
        m = self.good()
        a = m["apps"]["staff"]
        self.assertEqual(a["sha256"], self.sha)
        self.assertEqual(a["size"], len(b"hello dmg bytes"))
        self.assertEqual(a["rollout"], "test")
        self.assertEqual(a["url"], rm.DMG_URL_PREFIX + "57/Hope-Desk.dmg")
        self.assertEqual(rm.validate_manifest(m), [])

    def test_cli_write_prints_keys_and_headers(self):
        out = os.path.join(self.tmp, "latest.json")
        orig = rm.inspect_dmg
        rm.inspect_dmg = fake_inspect("hd2.57")
        rm.build_manifest.__defaults__ = (rm.inspect_dmg,)
        try:
            code, stdout, _ = run(["write", "--build", "57", "--dmg", "staff=" + self.dmg,
                                   "--test-id", "PP-Test-01", "--out", out])
        finally:
            rm.inspect_dmg = orig
            rm.build_manifest.__defaults__ = (orig,)
        self.assertEqual(code, 0)
        self.assertIn("hope-desk/releases/57/Hope-Desk.dmg", stdout)
        self.assertIn("hope-desk/latest.json", stdout)
        self.assertIn("application/json", stdout)
        self.assertIn("no-cache", stdout)
        self.assertEqual(run(["validate", out])[0], 0)

    def test_lower_build_number_in_dmg_refused(self):
        with self.assertRaises(rm.ManifestError):
            rm.build_manifest(57, {"staff": self.dmg}, [], "test", inspect=fake_inspect("hd2.56"))

    def test_older_build_does_not_overwrite_newer_manifest(self):
        out = self.write_file(self.good(), "latest.json")
        orig = rm.build_manifest.__defaults__
        rm.build_manifest.__defaults__ = (fake_inspect("hd2.50"),)
        try:
            code, _, err = run(["write", "--build", "50", "--dmg", "staff=" + self.dmg, "--out", out])
        finally:
            rm.build_manifest.__defaults__ = orig
        self.assertEqual(code, 1)
        self.assertIn("downgrades", err)

    def test_unknown_app_and_missing_file(self):
        with self.assertRaises(rm.ManifestError):
            rm.build_manifest(57, {"bogus": self.dmg}, [], "test", inspect=fake_inspect("hd2.57"))
        with self.assertRaises(rm.ManifestError):
            rm.build_manifest(57, {"staff": "/nope.dmg"}, [], "test", inspect=fake_inspect("hd2.57"))


class ValidateTests(Base):
    def errs(self, mutate):
        m = copy.deepcopy(self.good())
        mutate(m)
        return rm.validate_manifest(m)

    def test_example_file_valid(self):
        self.assertEqual(rm.validate_manifest(rm.load_manifest(os.path.join(HERE, "latest.example.json"))), [])

    def test_bad_hash(self):
        for bad in ("abc", "g" * 64, "a" * 63, "a" * 65, 12):
            self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("sha256", bad)), bad)

    def test_url_outside_prefix(self):
        bad = [
            "https://evil.example.com/hope-desk/releases/57/Hope-Desk.dmg",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/Hope-Desk.dmg",
            "http://onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/Hope-Desk.dmg",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/x.pkg",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/x.dmg?a=1",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/x.dmg#f",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/../56/x.dmg",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/56/x.dmg",
            "https://onehope-downloads.s3.amazonaws.com:443/hope-desk/releases/57/x.dmg",
            "https://u@onehope-downloads.s3.amazonaws.com/hope-desk/releases/57/x.dmg",
        ]
        for u in bad:
            self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("url", u)), u)

    def test_lower_build_vs_bundle_version(self):
        self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("bundle_version", "hd2.56")))
        self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("build", 0)))
        self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("build", -1)))

    def test_rollout_schema_size_fields(self):
        self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("rollout", "some")))
        self.assertTrue(self.errs(lambda m: m.__setitem__("schema", 2)))
        for size in (0, -5, 600 * 1024 * 1024 + 1, "9", True):
            self.assertTrue(self.errs(lambda m: m["apps"]["staff"].__setitem__("size", size)), size)
        self.assertTrue(self.errs(lambda m: m["apps"]["staff"].pop("sha256")))
        self.assertTrue(self.errs(lambda m: m["apps"].__setitem__("laptop", m["apps"]["staff"])))
        self.assertTrue(self.errs(lambda m: m.__setitem__("test_ids", "PP")))

    def test_malformed_manifest(self):
        for text in ("{not json", "", "[1,2]", '{"schema":1}'):
            p = self.write_file(text)
            code, _, err = run(["validate", p])
            self.assertEqual(code, 1, text)
            self.assertTrue(err)
        self.assertEqual(run(["validate", os.path.join(self.tmp, "missing.json")])[0], 1)
        p = self.write_file(" " * (65 * 1024))
        self.assertEqual(run(["validate", p])[0], 1)


class FlipTests(Base):
    def manifest_path(self):
        return self.write_file(self.good())

    def test_flip_without_confirmation_refused(self):
        p = self.manifest_path()
        code, _, err = run(["flip", "--app", "staff", "--to", "all", p])
        self.assertEqual(code, 1)
        self.assertIn("--i-confirmed-test-mac", err)
        self.assertEqual(rm.load_manifest(p)["apps"]["staff"]["rollout"], "test")

    def test_flip_with_confirmation(self):
        p = self.manifest_path()
        before = rm.load_manifest(p)
        code, out, _ = run(["flip", "--app", "staff", "--to", "all", "--i-confirmed-test-mac", p])
        self.assertEqual(code, 0)
        after = rm.load_manifest(p)
        self.assertEqual(after["apps"]["staff"]["rollout"], "all")
        after["apps"]["staff"]["rollout"] = "test"
        self.assertEqual(after, before)  # only rollout changed
        self.assertIn("hope-desk/Hope-Desk.dmg", out)

    def test_flip_missing_app_or_bad_manifest(self):
        p = self.manifest_path()
        self.assertEqual(run(["flip", "--app", "manager", "--to", "all", "--i-confirmed-test-mac", p])[0], 1)
        bad = self.write_file("{oops", "bad.json")
        self.assertEqual(run(["flip", "--app", "all", "--to", "all", "--i-confirmed-test-mac", bad])[0], 1)


@unittest.skipUnless(sys.platform == "darwin" and os.path.exists(rm.HDIUTIL) and os.path.exists(rm.PLISTBUDDY),
                     "needs macOS hdiutil/PlistBuddy")
class RealDmgTest(Base):
    def test_inspect_real_dmg(self):
        src = os.path.join(self.tmp, "src")
        app = os.path.join(src, "Hope Desk.app", "Contents")
        os.makedirs(app)
        with open(os.path.join(app, "Info.plist"), "w") as f:
            f.write('<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
                    "<key>CFBundleVersion</key><string>hd2.57</string>"
                    "<key>CFBundleShortVersionString</key><string>1.5.1</string></dict></plist>")
        dmg = os.path.join(self.tmp, "real", "Hope-Desk.dmg")
        os.makedirs(os.path.dirname(dmg))
        r = subprocess.run([rm.HDIUTIL, "create", "-quiet", "-srcfolder", src, "-volname", "T", dmg],
                           capture_output=True, text=True)
        if r.returncode != 0:
            self.skipTest("hdiutil create unavailable: " + r.stderr)
        self.assertEqual(rm.inspect_dmg("staff", dmg), ("hd2.57", "1.5.1"))
        m = rm.build_manifest(57, {"staff": dmg}, ["PP"], "test")
        self.assertEqual(rm.validate_manifest(m), [])
        with self.assertRaises(rm.ManifestError):
            rm.build_manifest(58, {"staff": dmg}, ["PP"], "test")


if __name__ == "__main__":
    unittest.main(verbosity=2)
