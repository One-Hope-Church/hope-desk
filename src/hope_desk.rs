//! One Hope Church build configuration ("Hope Desk").
//!
//! The Cargo feature `hope-desk-staff` selects the staff (privacy) build; without it the build is
//! Hope Desk Production. This module replaces upstream's RustDesk-Inc-signed `custom.txt`: the same
//! global setting maps are filled from compiled-in values, so no file can override them.

use base::config::keys;
use hbb_common::{
    config::{
        APP_NAME as HBB_APP_NAME, BUILTIN_SETTINGS, DEFAULT_DISPLAY_SETTINGS, DEFAULT_SETTINGS,
        HARD_SETTINGS, OVERWRITE_SETTINGS, PROD_RENDEZVOUS_SERVER,
    },
    log,
};
use std::{
    collections::HashMap,
    sync::{Once, RwLock},
};

pub const IS_STAFF: bool = cfg!(feature = "hope-desk-staff");

#[inline]
pub const fn is_staff() -> bool {
    IS_STAFF
}

pub const RENDEZVOUS_SERVER: &str = "remote.onehopechurch.com";
pub const RS_PUB_KEY: &str = "nQjpDmzFBATbFekm+4QeIJzU1fDGGyo5uURkU2MsLpY=";

/// Internal name: IPC paths, config dir, plists. Must stay space-free (`validate_install_app_name`).
pub const APP_NAME: &str = if IS_STAFF {
    "HopeDesk"
} else {
    "HopeDeskProduction"
};
/// User-visible name (window titles, translations).
pub const DISPLAY_NAME: &str = if IS_STAFF {
    "Hope Desk"
} else {
    "Hope Desk Production"
};

/// Hard option Flutter reads through `mainGetHardOption` to learn the variant.
pub const HARD_OPTION_STAFF: &str = "hope-desk-staff";

pub fn display_name() -> String {
    DISPLAY_NAME.to_owned()
}

fn fill(map: &RwLock<HashMap<String, String>>, entries: &[(&str, &str)]) {
    let mut m = map.write().unwrap();
    for (k, v) in entries {
        m.insert((*k).to_owned(), (*v).to_owned());
    }
}

pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        *HBB_APP_NAME.write().unwrap() = APP_NAME.to_owned();
        *PROD_RENDEZVOUS_SERVER.write().unwrap() = RENDEZVOUS_SERVER.to_owned();

        let mut overwrite = vec![
            (keys::OPTION_CUSTOM_RENDEZVOUS_SERVER, RENDEZVOUS_SERVER),
            (keys::OPTION_RELAY_SERVER, RENDEZVOUS_SERVER),
            (keys::OPTION_KEY, RS_PUB_KEY),
            (keys::OPTION_API_SERVER, ""),
            (keys::OPTION_ENABLE_CHECK_UPDATE, "N"),
            (keys::OPTION_ALLOW_AUTO_UPDATE, "N"),
        ];
        let mut default_settings = vec![(keys::OPTION_ENABLE_HWCODEC, "Y")];
        let mut builtin = vec![
            (keys::OPTION_REGISTER_DEVICE, "N"),
            (keys::OPTION_HIDE_SERVER_SETTINGS, "Y"),
            (keys::OPTION_HIDE_POWERED_BY_ME, "Y"),
        ];
        // No constants exist for these keys.
        let mut hard = vec![("disable-ab", "Y"), ("disable-account", "Y")];
        if IS_STAFF {
            overwrite.extend([
                ("verification-method", "use-temporary-password"),
                (keys::OPTION_APPROVE_MODE, ""),
                ("allow-hide-cm", "N"),
                (keys::OPTION_ALLOW_REMOTE_CONFIG_MODIFICATION, "N"),
                (keys::OPTION_DIRECT_SERVER, "N"),
            ]);
            builtin.extend([
                (keys::OPTION_DISABLE_CHANGE_PERMANENT_PASSWORD, "Y"),
                (keys::OPTION_HIDE_SECURITY_SETTINGS, "Y"),
                (keys::OPTION_HIDE_STOP_SERVICE, "Y"),
            ]);
            hard.extend([(HARD_OPTION_STAFF, "Y"), ("disable-installation", "Y")]);
        } else {
            default_settings.extend([
                (keys::OPTION_APPROVE_MODE, "password"),
                (keys::OPTION_ALLOW_REMOTE_CONFIG_MODIFICATION, "Y"),
            ]);
        }
        fill(&OVERWRITE_SETTINGS, &overwrite);
        fill(&DEFAULT_SETTINGS, &default_settings);
        fill(
            &DEFAULT_DISPLAY_SETTINGS,
            &[(keys::OPTION_IMAGE_QUALITY, "low")],
        );
        fill(&BUILTIN_SETTINGS, &builtin);
        fill(&HARD_SETTINGS, &hard);
    });
}

/// Returns false when the staff build must refuse the command line.
pub fn check_cli_args(args: &[String]) -> bool {
    if !IS_STAFF {
        return true;
    }
    const ALLOWED: &[&str] = &[
        "--connect",
        "--play",
        "--file-transfer",
        "--view-camera",
        "--port-forward",
        "--terminal",
        "--rdp",
        "--cm",
        "--whiteboard",
        "--check-hwcodec-config",
        "--version",
        "--build-date",
        "--get-id",
        "--uninstall-service",
    ];
    let Some(arg) = args.first() else {
        return true;
    };
    if arg.starts_with("--") && !ALLOWED.contains(&arg.as_str()) {
        eprintln!("{} is not available in Hope Desk", arg);
        log::warn!("{} is not available in Hope Desk", arg);
        return false;
    }
    if matches!(arg.as_str(), "--cm" | "--whiteboard" | "--check-hwcodec-config") {
        start_orphan_watchdog();
    }
    true
}

/// Child processes (connection manager, whiteboard, codec check) exit once the GUI that spawned
/// them is gone, so quitting Hope Desk leaves nothing running.
fn start_orphan_watchdog() {
    #[cfg(unix)]
    {
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if std::os::unix::process::parent_id() == 1 {
                log::info!("Parent process is gone, exiting");
                std::process::exit(0);
            }
        });
    }
}
