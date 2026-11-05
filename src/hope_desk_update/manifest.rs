//! Hope Desk update manifest: fetching, strict validation, version comparison and rollout gating.
//!
//! Pure logic lives here so it can be unit-tested without a network. The manifest is NOT a trust
//! anchor: it only pins the SHA-256 and tells us which build exists. The real gate is the Developer
//! ID / notarization check in `verify.rs`.

use crate::hope_desk::Variant;
use hbb_common::{
    bail,
    config::Config,
    log,
    serde_json::{self, Value},
    ResultType,
};
use std::{collections::HashMap, io::Read, time::Duration};

pub const MANIFEST_URL: &str = "https://onehope-downloads.s3.amazonaws.com/hope-desk/latest.json";
pub const DMG_URL_PREFIX: &str = "https://onehope-downloads.s3.amazonaws.com/hope-desk/releases/";

const SCHEMA: u64 = 1;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_DMG_BYTES: u64 = 600 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const APP_KEYS: [&str; 3] = ["staff", "production", "manager"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rollout {
    Test,
    All,
}

#[derive(Clone, Debug)]
pub struct AppRelease {
    pub build: u64,
    pub short_version: String,
    pub bundle_version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub rollout: Rollout,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub test_ids: Vec<String>,
    pub apps: HashMap<String, AppRelease>,
}

/// Parse and validate a manifest. Any structural problem is an `Err` (the caller treats it as "no
/// update"). Behaviour worth knowing:
/// - an app key other than staff/production/manager is ignored;
/// - an app entry whose `rollout` is not "test" or "all" is dropped (that app gets no update);
/// - any other invalid field in a known app entry fails the whole manifest.
pub fn parse_manifest(bytes: &[u8]) -> ResultType<Manifest> {
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        bail!("manifest too large ({} bytes)", bytes.len());
    }
    let root: Value = serde_json::from_slice(bytes)?;
    let obj = match root.as_object() {
        Some(o) => o,
        None => bail!("manifest is not a JSON object"),
    };
    match obj.get("schema").and_then(Value::as_u64) {
        Some(SCHEMA) => {}
        other => bail!("unsupported manifest schema: {:?}", other),
    }
    let mut test_ids = Vec::new();
    match obj.get("test_ids") {
        None | Some(Value::Null) => {}
        Some(Value::Array(a)) => {
            for v in a {
                match v.as_str() {
                    Some(s) => test_ids.push(s.to_owned()),
                    None => bail!("test_ids must contain only strings"),
                }
            }
        }
        Some(_) => bail!("test_ids must be an array"),
    }
    let apps_obj = match obj.get("apps").and_then(Value::as_object) {
        Some(a) => a,
        None => bail!("manifest has no apps object"),
    };
    let mut apps = HashMap::new();
    for key in APP_KEYS {
        if let Some(entry) = apps_obj.get(key) {
            match parse_app(key, entry)? {
                Some(rel) => {
                    apps.insert(key.to_owned(), rel);
                }
                None => log::info!("hope-desk update: {} has unknown rollout, ignoring", key),
            }
        }
    }
    Ok(Manifest { test_ids, apps })
}

fn parse_app(key: &str, v: &Value) -> ResultType<Option<AppRelease>> {
    let o = match v.as_object() {
        Some(o) => o,
        None => bail!("{}: entry is not an object", key),
    };
    let str_field = |name: &str| -> ResultType<String> {
        match o.get(name).and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => Ok(s.to_owned()),
            _ => bail!("{}: missing or invalid '{}'", key, name),
        }
    };
    let u64_field = |name: &str| -> ResultType<u64> {
        match o.get(name).and_then(Value::as_u64) {
            Some(n) => Ok(n),
            None => bail!("{}: missing or invalid '{}'", key, name),
        }
    };
    let build = u64_field("build")?;
    if build == 0 {
        bail!("{}: build must be > 0", key);
    }
    let size = u64_field("size")?;
    if size == 0 || size > MAX_DMG_BYTES {
        bail!("{}: size out of range ({})", key, size);
    }
    let short_version = str_field("short_version")?;
    let bundle_version = str_field("bundle_version")?;
    let url = str_field("url")?;
    validate_dmg_url(&url).map_err(|e| hbb_common::anyhow::anyhow!("{}: {}", key, e))?;
    let sha256 = str_field("sha256")?.to_ascii_lowercase();
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{}: sha256 must be 64 hex characters", key);
    }
    let rollout = match str_field("rollout")?.as_str() {
        "test" => Rollout::Test,
        "all" => Rollout::All,
        _ => return Ok(None),
    };
    Ok(Some(AppRelease {
        build,
        short_version,
        bundle_version,
        url,
        sha256,
        size,
        rollout,
    }))
}

/// URL rule: compiled-in prefix, `.dmg` suffix, no query, fragment, userinfo, port or `..`.
pub fn validate_dmg_url(url: &str) -> ResultType<()> {
    let rest = match url.strip_prefix(DMG_URL_PREFIX) {
        Some(r) if !r.is_empty() => r,
        _ => bail!("url is not under {}", DMG_URL_PREFIX),
    };
    if !rest.ends_with(".dmg") {
        bail!("url must end in .dmg");
    }
    // The host is fixed by the prefix, so userinfo/port can only appear if smuggled into the path.
    if rest.contains("..")
        || rest.contains('?')
        || rest.contains('#')
        || rest.contains('@')
        || rest.contains(':')
        || rest.contains('\\')
        || rest.contains('%')
        || rest.contains("//")
        || rest.bytes().any(|b| b < 0x21 || b > 0x7e)
    {
        bail!("url contains a forbidden character or sequence");
    }
    Ok(())
}

/// Blocking fetch of the manifest through the strict (HTTPS-only) client: 15 s timeout, 64 KiB cap.
pub fn fetch_manifest() -> ResultType<Manifest> {
    let client = crate::hbbs_http::create_http_client_with_url_strict(MANIFEST_URL)?;
    let response = client
        .get(MANIFEST_URL)
        .header("Cache-Control", "no-cache")
        .timeout(FETCH_TIMEOUT)
        .send()?;
    if !response.status().is_success() {
        bail!("manifest fetch failed: {}", response.status());
    }
    let mut body = Vec::new();
    response
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut body)?;
    parse_manifest(&body)
}

/// This build's CI run number, or `None` for a local build (which never auto-updates).
pub fn own_build() -> Option<u64> {
    option_env!("HOPE_DESK_BUILD_NUMBER").and_then(|s| s.trim().parse::<u64>().ok())
}

pub fn variant_key() -> &'static str {
    match crate::hope_desk::VARIANT {
        Variant::Staff => "staff",
        Variant::Production => "production",
        Variant::Manager => "manager",
    }
}

pub fn own_bundle_id() -> &'static str {
    match crate::hope_desk::VARIANT {
        Variant::Staff => "org.onehopechurch.hopedesk",
        Variant::Production => "org.onehopechurch.hopedesk.production",
        Variant::Manager => "org.onehopechurch.hopedesk.manager",
    }
}

pub fn shipped_bundle_name() -> &'static str {
    match crate::hope_desk::VARIANT {
        Variant::Staff => "Hope Desk.app",
        Variant::Production => "HopeDeskProduction.app",
        Variant::Manager => "Hope Desk Manager.app",
    }
}

/// Identifiers that can match `test_ids`: the Hope Desk ID and the computer name.
pub fn rollout_ids() -> Vec<String> {
    let mut ids = vec![Config::get_id()];
    ids.push(hbb_common::whoami::devicename());
    ids.retain(|s| !s.trim().is_empty());
    ids
}

/// The release this Mac should install, if any: the app's entry exists, its build is strictly newer
/// than ours, it's not a known-failed build, and the rollout gate admits this Mac.
pub fn eligible_release(
    m: &Manifest,
    ids: &[String],
    failed_builds: &[u64],
) -> Option<AppRelease> {
    eligible_release_for(m, ids, failed_builds, own_build(), variant_key())
}

pub(crate) fn eligible_release_for(
    m: &Manifest,
    ids: &[String],
    failed_builds: &[u64],
    own_build: Option<u64>,
    key: &str,
) -> Option<AppRelease> {
    let own = own_build?;
    let rel = m.apps.get(key)?;
    if rel.build <= own {
        return None;
    }
    if failed_builds.contains(&rel.build) {
        log::info!("hope-desk update: build {} previously failed, skipping", rel.build);
        return None;
    }
    match rel.rollout {
        Rollout::All => Some(rel.clone()),
        Rollout::Test => {
            let listed = ids.iter().any(|id| {
                m.test_ids
                    .iter()
                    .any(|t| t.trim().eq_ignore_ascii_case(id.trim()))
            });
            if listed {
                Some(rel.clone())
            } else {
                log::info!("hope-desk update: not in test list");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn app_json(build: u64, rollout: &str) -> String {
        format!(
            r#"{{"build":{b},"short_version":"1.5.1","bundle_version":"hd2.{b}","rollout":"{r}",
            "url":"{p}{b}/Hope-Desk.dmg","sha256":"{h}","size":1000}}"#,
            b = build,
            r = rollout,
            p = DMG_URL_PREFIX,
            h = HASH
        )
    }

    fn manifest_json(staff: &str) -> Vec<u8> {
        format!(
            r#"{{"schema":1,"test_ids":["PP-Test-01","IT-MacBook-Pro"],"apps":{{"staff":{}}}}}"#,
            staff
        )
        .into_bytes()
    }

    fn with_field(field: &str, value: &str) -> Vec<u8> {
        let mut v: Value = serde_json::from_str(&app_json(58, "all")).unwrap();
        v[field] = serde_json::from_str(value).unwrap();
        manifest_json(&v.to_string())
    }

    #[test]
    fn parses_valid() {
        let m = parse_manifest(&manifest_json(&app_json(58, "test"))).unwrap();
        assert_eq!(m.test_ids.len(), 2);
        let r = &m.apps["staff"];
        assert_eq!(r.build, 58);
        assert_eq!(r.rollout, Rollout::Test);
        assert_eq!(r.bundle_version, "hd2.58");
    }

    #[test]
    fn rejects_bad_schema_and_garbage() {
        assert!(parse_manifest(b"{").is_err());
        assert!(parse_manifest(b"[]").is_err());
        assert!(parse_manifest(br#"{"schema":2,"apps":{}}"#).is_err());
        assert!(parse_manifest(br#"{"apps":{}}"#).is_err());
        assert!(parse_manifest(br#"{"schema":1}"#).is_err());
        assert!(parse_manifest(&vec![b' '; 70 * 1024]).is_err());
    }

    #[test]
    fn rejects_invalid_fields() {
        assert!(parse_manifest(&with_field("build", "0")).is_err());
        assert!(parse_manifest(&with_field("build", "\"58\"")).is_err());
        assert!(parse_manifest(&with_field("size", "0")).is_err());
        assert!(parse_manifest(&with_field("size", "629145601")).is_err());
        assert!(parse_manifest(&with_field("sha256", "\"abc\"")).is_err());
        assert!(parse_manifest(&with_field("sha256", &format!("\"{}z\"", &HASH[..63]))).is_err());
        assert!(parse_manifest(&with_field("short_version", "\"\"")).is_err());
        let bad_ids = br#"{"schema":1,"test_ids":"PP","apps":{}}"#;
        assert!(parse_manifest(bad_ids).is_err());
    }

    #[test]
    fn missing_field_rejected() {
        let mut v: Value = serde_json::from_str(&app_json(58, "all")).unwrap();
        v.as_object_mut().unwrap().remove("sha256");
        assert!(parse_manifest(&manifest_json(&v.to_string())).is_err());
    }

    #[test]
    fn unknown_rollout_drops_app() {
        let m = parse_manifest(&manifest_json(&app_json(58, "canary"))).unwrap();
        assert!(m.apps.is_empty());
    }

    #[test]
    fn url_prefix_rule() {
        let ok = format!("{}58/Hope-Desk.dmg", DMG_URL_PREFIX);
        assert!(validate_dmg_url(&ok).is_ok());
        for bad in [
            "http://onehope-downloads.s3.amazonaws.com/hope-desk/releases/58/a.dmg",
            "https://evil.example.com/hope-desk/releases/58/a.dmg",
            "https://onehope-downloads.s3.amazonaws.com/hope-desk/Hope-Desk.dmg",
            &format!("{}58/a.zip", DMG_URL_PREFIX),
            &format!("{}58/a.dmg?x=1", DMG_URL_PREFIX),
            &format!("{}58/a.dmg#f", DMG_URL_PREFIX),
            &format!("{}58/../../x.dmg", DMG_URL_PREFIX),
            &format!("{}x@evil.com/a.dmg", DMG_URL_PREFIX),
            &format!("{}58:80/a.dmg", DMG_URL_PREFIX),
            DMG_URL_PREFIX,
        ] {
            assert!(validate_dmg_url(bad).is_err(), "should reject {}", bad);
        }
        let bad_url = with_field("url", "\"https://evil.example.com/a.dmg\"");
        assert!(parse_manifest(&bad_url).is_err());
    }

    fn manifest_with(build: u64, rollout: &str) -> Manifest {
        parse_manifest(&manifest_json(&app_json(build, rollout))).unwrap()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn version_compare() {
        let m = manifest_with(58, "all");
        assert!(eligible_release_for(&m, &[], &[], Some(57), "staff").is_some());
        assert!(eligible_release_for(&m, &[], &[], Some(58), "staff").is_none());
        assert!(eligible_release_for(&m, &[], &[], Some(59), "staff").is_none());
        // local build never updates
        assert!(eligible_release_for(&m, &[], &[], None, "staff").is_none());
        // app missing from manifest
        assert!(eligible_release_for(&m, &[], &[], Some(1), "manager").is_none());
    }

    #[test]
    fn failed_builds_skipped() {
        let m = manifest_with(58, "all");
        assert!(eligible_release_for(&m, &[], &[58], Some(57), "staff").is_none());
        assert!(eligible_release_for(&m, &[], &[57], Some(57), "staff").is_some());
    }

    #[test]
    fn test_id_gate() {
        let m = manifest_with(58, "test");
        assert!(eligible_release_for(&m, &ids(&["other"]), &[], Some(57), "staff").is_none());
        assert!(eligible_release_for(&m, &[], &[], Some(57), "staff").is_none());
        assert!(eligible_release_for(&m, &ids(&["pp-test-01"]), &[], Some(57), "staff").is_some());
        // computer name, case-insensitive
        assert!(eligible_release_for(
            &m,
            &ids(&["123 456 789", "IT-MACBOOK-PRO"]),
            &[],
            Some(57),
            "staff"
        )
        .is_some());
    }
}
