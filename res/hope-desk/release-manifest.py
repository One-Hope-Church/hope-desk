#!/usr/bin/env python3
"""Hope Desk release manifest tool (v2.1 Chunk C). Python 3 standard library only.

  release-manifest.py write --build N --dmg staff=Hope-Desk.dmg \
        --dmg production=Hope-Desk-Production.dmg --dmg manager=Hope-Desk-Manager.dmg \
        --test-id PP-Test-01 --test-id IT-MacBook-Pro [--rollout test] [--out latest.json]
  release-manifest.py flip --app production|staff|manager|all --to all \
        --i-confirmed-test-mac latest.json
  release-manifest.py validate latest.json

No AWS calls, no credentials. Uploads go through the existing presigned-PUT flow;
this tool only prints the S3 keys and headers to use.

validate_manifest() mirrors the rules in manifest.rs (technical-plan 2.1.1) and adds
release-tooling checks (bundle_version == hd2.<build>, URL under releases/<build>/).
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from urllib.parse import urlsplit

BASE = "https://onehope-downloads.s3.amazonaws.com/"
DMG_URL_PREFIX = BASE + "hope-desk/releases/"
MAX_MANIFEST_BYTES = 64 * 1024
MAX_DMG_BYTES = 600 * 1024 * 1024  # 600 MB (MiB; keep identical to the Rust constant)
APPS = ("staff", "production", "manager")
ROLLOUTS = ("test", "all")
BUNDLE_NAMES = {
    "staff": "Hope Desk.app",
    "production": "HopeDeskProduction.app",
    "manager": "Hope Desk Manager.app",
}
INSTALL_LINK_NAMES = {
    "staff": "Hope-Desk.dmg",
    "production": "Hope-Desk-Production.dmg",
    "manager": "Hope-Desk-Manager.dmg",
}
HEX64 = re.compile(r"^[0-9a-fA-F]{64}$")
PLISTBUDDY = "/usr/libexec/PlistBuddy"
HDIUTIL = "/usr/bin/hdiutil"


class ManifestError(Exception):
    pass


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def validate_url(url, build):
    """Same checks as the client, plus the per-build directory."""
    if not isinstance(url, str) or not url.startswith(DMG_URL_PREFIX):
        return "url must start with " + DMG_URL_PREFIX
    if not url.endswith(".dmg"):
        return "url must end in .dmg"
    if any(c in url for c in "?#@\\") or ".." in url or any(ord(c) < 33 for c in url):
        return "url must not contain query, fragment, userinfo, '..', backslash or whitespace"
    parts = urlsplit(url)
    if parts.query or parts.fragment or parts.username is not None or parts.password is not None:
        return "url must not contain query, fragment or userinfo"
    if parts.port is not None or parts.netloc != "onehope-downloads.s3.amazonaws.com":
        return "url must not contain a port or other host"
    rest = url[len(DMG_URL_PREFIX):].split("/")
    if len(rest) != 2 or not rest[1] or rest[0] != str(build):
        return "url must be hope-desk/releases/%s/<name>.dmg" % build
    return None


def validate_manifest(m):
    """Return a list of error strings (empty means valid)."""
    errs = []
    if not isinstance(m, dict):
        return ["manifest must be a JSON object"]
    if m.get("schema") != 1 or not _is_int(m.get("schema")):
        errs.append("schema must be 1")
    ids = m.get("test_ids")
    if not isinstance(ids, list) or not all(isinstance(i, str) and i.strip() for i in ids):
        errs.append("test_ids must be a list of non-empty strings")
    apps = m.get("apps")
    if not isinstance(apps, dict):
        errs.append("apps must be an object")
        return errs
    for name, a in apps.items():
        p = "apps.%s: " % name
        if name not in APPS:
            errs.append(p + "unknown app (expected one of %s)" % ", ".join(APPS))
            continue
        if not isinstance(a, dict):
            errs.append(p + "must be an object")
            continue
        build = a.get("build")
        if not _is_int(build) or build <= 0:
            errs.append(p + "build must be an integer > 0")
            build = None
        for f in ("short_version", "bundle_version"):
            if not isinstance(a.get(f), str) or not a.get(f).strip():
                errs.append(p + "%s must be a non-empty string" % f)
        bv = a.get("bundle_version")
        if build is not None and isinstance(bv, str) and bv != "hd2.%d" % build:
            errs.append(p + "bundle_version must be hd2.%d" % build)
        if a.get("rollout") not in ROLLOUTS:
            errs.append(p + "rollout must be one of %s" % ", ".join(ROLLOUTS))
        sha = a.get("sha256")
        if not isinstance(sha, str) or not HEX64.match(sha):
            errs.append(p + "sha256 must be 64 hex characters")
        size = a.get("size")
        if not _is_int(size) or not (0 < size <= MAX_DMG_BYTES):
            errs.append(p + "size must be an integer, 0 < size <= %d" % MAX_DMG_BYTES)
        if build is not None:
            e = validate_url(a.get("url"), build)
            if e:
                errs.append(p + e)
        elif not isinstance(a.get("url"), str):
            errs.append(p + "url must be a string")
    return errs


def load_manifest(path):
    try:
        with open(path, "rb") as f:
            raw = f.read(MAX_MANIFEST_BYTES + 1)
    except OSError as e:
        raise ManifestError("cannot read %s: %s" % (path, e))
    if len(raw) > MAX_MANIFEST_BYTES:
        raise ManifestError("manifest larger than 64 KiB")
    try:
        return json.loads(raw.decode("utf-8"))
    except (ValueError, UnicodeDecodeError) as e:
        raise ManifestError("malformed JSON: %s" % e)


def dump_manifest(m):
    text = json.dumps(m, indent=2) + "\n"
    if len(text.encode()) > MAX_MANIFEST_BYTES:
        raise ManifestError("manifest larger than 64 KiB")
    return text


def sha256_and_size(path):
    h = hashlib.sha256()
    n = 0
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
            n += len(chunk)
    return h.hexdigest(), n


def _plist(app, key):
    out = subprocess.run([PLISTBUDDY, "-c", "Print :" + key, os.path.join(app, "Contents", "Info.plist")],
                         capture_output=True, text=True)
    if out.returncode != 0:
        raise ManifestError("PlistBuddy could not read %s from %s" % (key, app))
    return out.stdout.strip()


def inspect_dmg(app_key, dmg):
    """Mount the DMG read-only and return (CFBundleVersion, CFBundleShortVersionString)."""
    with tempfile.TemporaryDirectory(prefix="hd-manifest-") as tmp:
        mnt = os.path.join(tmp, "mnt")
        os.mkdir(mnt)
        r = subprocess.run([HDIUTIL, "attach", "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mnt, dmg],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise ManifestError("hdiutil attach failed for %s: %s" % (dmg, r.stderr.strip()))
        try:
            app = os.path.join(mnt, BUNDLE_NAMES[app_key])
            if not os.path.isdir(app):
                raise ManifestError("%s does not contain %s" % (dmg, BUNDLE_NAMES[app_key]))
            return _plist(app, "CFBundleVersion"), _plist(app, "CFBundleShortVersionString")
        finally:
            subprocess.run([HDIUTIL, "detach", "-force", mnt], capture_output=True)


def build_manifest(build, dmgs, test_ids, rollout, inspect=inspect_dmg):
    apps = {}
    for key, path in dmgs.items():
        if key not in APPS:
            raise ManifestError("unknown app '%s'" % key)
        if not os.path.isfile(path):
            raise ManifestError("DMG not found: %s" % path)
        bundle_version, short_version = inspect(key, path)
        if bundle_version != "hd2.%d" % build:
            raise ManifestError("%s: CFBundleVersion is '%s', expected hd2.%d" % (path, bundle_version, build))
        sha, size = sha256_and_size(path)
        apps[key] = {
            "build": build,
            "short_version": short_version,
            "bundle_version": bundle_version,
            "rollout": rollout,
            "url": "%s%d/%s" % (DMG_URL_PREFIX, build, os.path.basename(path)),
            "sha256": sha,
            "size": size,
        }
    m = {"schema": 1, "test_ids": list(test_ids), "apps": apps}
    errs = validate_manifest(m)
    if errs:
        raise ManifestError("generated manifest is invalid:\n  " + "\n  ".join(errs))
    return m


def upload_plan(m, flipped_all=()):
    """Lines telling the operator exactly what to PUT (via the presigned flow)."""
    lines = ["Upload these objects (presigned PUT; bucket onehope-downloads):"]
    for key, a in m["apps"].items():
        s3 = a["url"][len(BASE):]
        lines.append("  %-52s Content-Type: application/x-apple-diskimage" % s3)
        lines.append("  %-52s Cache-Control: public, max-age=31536000, immutable" % "")
    lines.append("  %-52s Content-Type: application/json" % "hope-desk/latest.json")
    lines.append("  %-52s Cache-Control: no-cache, max-age=0" % "")
    lines.append("Upload latest.json LAST. Verify: curl -sI %shope-desk/latest.json | grep -i cache-control" % BASE)
    if flipped_all:
        lines.append("Only after the flip to 'all', PUT the same signed DMGs to the install-link names "
                     "(Content-Type application/x-apple-diskimage, Cache-Control: no-cache, max-age=0; "
                     "these names are overwritten every release, so never mark them immutable):")
        for key in flipped_all:
            lines.append("  hope-desk/" + INSTALL_LINK_NAMES[key])
    return "\n".join(lines)


def cmd_write(args):
    dmgs = {}
    for spec in args.dmg:
        if "=" not in spec:
            raise ManifestError("--dmg expects app=path, got '%s'" % spec)
        k, p = spec.split("=", 1)
        if k in dmgs:
            raise ManifestError("duplicate --dmg for %s" % k)
        dmgs[k] = p
    if not dmgs:
        raise ManifestError("at least one --dmg is required")
    m = build_manifest(args.build, dmgs, args.test_id, args.rollout)
    if os.path.exists(args.out):  # run numbers only go up; never overwrite with an older build
        try:
            old = load_manifest(args.out)
            old_builds = [a["build"] for a in old.get("apps", {}).values() if _is_int(a.get("build"))]
        except (ManifestError, AttributeError, TypeError):
            old_builds = []
        if old_builds and max(old_builds) > args.build:
            raise ManifestError("existing %s has build %d, higher than --build %d (downgrades are not allowed)"
                                % (args.out, max(old_builds), args.build))
    with open(args.out, "w") as f:
        f.write(dump_manifest(m))
    print("wrote %s (build %d, rollout %s, apps: %s)" % (args.out, args.build, args.rollout, ", ".join(m["apps"])))
    print(upload_plan(m))


def cmd_flip(args):
    if not args.i_confirmed_test_mac:
        raise ManifestError("refusing to flip: pass --i-confirmed-test-mac once the test Mac has checked out")
    m = load_manifest(args.manifest)
    errs = validate_manifest(m)
    if errs:
        raise ManifestError("input manifest invalid:\n  " + "\n  ".join(errs))
    targets = list(m["apps"]) if args.app == "all" else [args.app]
    for t in targets:
        if t not in m["apps"]:
            raise ManifestError("app '%s' is not in the manifest" % t)
    for t in targets:
        m["apps"][t]["rollout"] = args.to
    errs = validate_manifest(m)
    if errs:
        raise ManifestError("result invalid:\n  " + "\n  ".join(errs))
    with open(args.manifest, "w") as f:
        f.write(dump_manifest(m))
    print("flipped %s to rollout '%s' in %s" % (", ".join(targets), args.to, args.manifest))
    print(upload_plan(m, flipped_all=targets))


def cmd_validate(args):
    m = load_manifest(args.manifest)
    errs = validate_manifest(m)
    if errs:
        raise ManifestError("invalid:\n  " + "\n  ".join(errs))
    print("OK: %s is valid (%s)" % (args.manifest, ", ".join("%s build %d %s" % (k, a["build"], a["rollout"])
                                                           for k, a in m["apps"].items())))


def main(argv=None):
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    w = sub.add_parser("write")
    w.add_argument("--build", type=int, required=True)
    w.add_argument("--dmg", action="append", default=[], metavar="APP=PATH")
    w.add_argument("--test-id", action="append", default=[])
    w.add_argument("--rollout", choices=["test"], default="test",
                   help="write only produces 'test'; use flip to go to 'all'")
    w.add_argument("--out", default="latest.json")
    f = sub.add_parser("flip")
    f.add_argument("--app", required=True, choices=list(APPS) + ["all"])
    f.add_argument("--to", required=True, choices=["all"])
    f.add_argument("--i-confirmed-test-mac", action="store_true")
    f.add_argument("manifest")
    v = sub.add_parser("validate")
    v.add_argument("manifest")
    args = p.parse_args(argv)
    try:
        {"write": cmd_write, "flip": cmd_flip, "validate": cmd_validate}[args.cmd](args)
    except ManifestError as e:
        print("error: %s" % e, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
