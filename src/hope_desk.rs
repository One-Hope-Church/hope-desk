//! One Hope Church build configuration ("Hope Desk").
//!
//! Three variants: the Cargo feature `hope-desk-staff` selects the staff (privacy) build,
//! `hope-desk-manager` selects Hope Desk Manager (connects out, never accepts), and neither selects
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

#[cfg(all(feature = "hope-desk-staff", feature = "hope-desk-manager"))]
compile_error!("hope-desk-staff and hope-desk-manager are mutually exclusive");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    Staff,
    Production,
    Manager,
}

pub const VARIANT: Variant = if cfg!(feature = "hope-desk-staff") {
    Variant::Staff
} else if cfg!(feature = "hope-desk-manager") {
    Variant::Manager
} else {
    Variant::Production
};

pub const IS_STAFF: bool = matches!(VARIANT, Variant::Staff);

#[inline]
pub const fn is_staff() -> bool {
    IS_STAFF
}

#[inline]
pub const fn is_production() -> bool {
    matches!(VARIANT, Variant::Production)
}

#[inline]
pub const fn is_manager() -> bool {
    matches!(VARIANT, Variant::Manager)
}

/// Staff and Production: may never open an outgoing session.
#[inline]
pub const fn is_receive_only() -> bool {
    !is_manager()
}

/// Only Production may install/run the launchd service.
#[inline]
pub const fn allows_service() -> bool {
    is_production()
}

pub const RENDEZVOUS_SERVER: &str = "remote.onehopechurch.com";
pub const RS_PUB_KEY: &str = "nQjpDmzFBATbFekm+4QeIJzU1fDGGyo5uURkU2MsLpY=";

/// Internal name: IPC paths, config dir, plists. Must stay space-free (`validate_install_app_name`).
pub const APP_NAME: &str = match VARIANT {
    Variant::Staff => "HopeDesk",
    Variant::Production => "HopeDeskProduction",
    Variant::Manager => "HopeDeskManager",
};
/// User-visible name (window titles, translations).
pub const DISPLAY_NAME: &str = match VARIANT {
    Variant::Staff => "Hope Desk",
    Variant::Production => "Hope Desk Production",
    Variant::Manager => "Hope Desk Manager",
};

/// Install links Hope Desk Manager copies for staff to download (DMG names are fixed).
pub const STAFF_DMG_URL: &str = "https://onehope-downloads.s3.amazonaws.com/hope-desk/Hope-Desk.dmg";
pub const PRODUCTION_DMG_URL: &str =
    "https://onehope-downloads.s3.amazonaws.com/hope-desk/Hope-Desk-Production.dmg";
/// Polls (0.3 s apart) a new connection waits for the connection manager to start listening.
/// Upstream waits 20 (~6 s); a background or cold-starting Flutter CM can take longer, and a
/// connection that gives up is left with no Accept prompt at all.
pub const CM_START_POLLS: usize = 200;
/// Hope Desk release number shown after the upstream protocol version.
pub const HOPE_DESK_RELEASE: &str = "2";

/// Hard option Flutter reads through `mainGetHardOption` to learn the variant.
pub const HARD_OPTION_STAFF: &str = "hope-desk-staff";

/// e.g. "1.5.1 · Hope Desk 2 (build 37)". Display only; `crate::VERSION` stays the protocol version.
pub fn version_label() -> String {
    format!(
        "{} · Hope Desk {} ({})",
        crate::VERSION,
        HOPE_DESK_RELEASE,
        match option_env!("HOPE_DESK_BUILD_NUMBER") {
            Some(n) => format!("build {}", n),
            None => "local build".to_owned(),
        }
    )
}

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
        let mut hard = vec![
            ("disable-ab", "Y"),
            ("disable-account", "Y"),
            (
                "hope-desk-variant",
                match VARIANT {
                    Variant::Staff => "staff",
                    Variant::Production => "production",
                    Variant::Manager => "manager",
                },
            ),
            ("hope-desk-staff-dmg-url", STAFF_DMG_URL),
            ("hope-desk-production-dmg-url", PRODUCTION_DMG_URL),
            ("conn-type", if is_manager() { "outgoing" } else { "incoming" }),
        ];
        // Every variant lists itself under Input Monitoring and shows the permission card: a staff
        // Mac without it reported keys not arriving while mouse input worked.
        hard.push(("hope-desk-needs-input-monitoring", "Y"));
        match VARIANT {
            Variant::Staff => {
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
            }
            Variant::Production => {
                overwrite.extend([
                    (keys::OPTION_APPROVE_MODE, "password"),
                    ("verification-method", "use-permanent-password"),
                ]);
                default_settings.push((keys::OPTION_ALLOW_REMOTE_CONFIG_MODIFICATION, "Y"));
            }
            Variant::Manager => {
                // There is no server in Manager, so these are inert belts.
                overwrite.extend([
                    (keys::OPTION_DIRECT_SERVER, "N"),
                    ("verification-method", "use-temporary-password"),
                ]);
                builtin.extend([
                    (keys::OPTION_HIDE_SECURITY_SETTINGS, "Y"),
                    (keys::OPTION_HIDE_STOP_SERVICE, "Y"),
                    (keys::OPTION_DISABLE_CHANGE_PERMANENT_PASSWORD, "Y"),
                    ("disable-change-id", "Y"),
                ]);
                hard.push(("disable-installation", "Y"));
            }
        }
        fill(&OVERWRITE_SETTINGS, &overwrite);
        fill(&DEFAULT_SETTINGS, &default_settings);
        fill(
            &DEFAULT_DISPLAY_SETTINGS,
            &[(keys::OPTION_IMAGE_QUALITY, "low")],
        );
        fill(&BUILTIN_SETTINGS, &builtin);
        fill(&HARD_SETTINGS, &hard);
        log::info!("Hope Desk {:?} {}", VARIANT, version_label());
    });
}

const OUTGOING_FLAGS: &[&str] = &[
    "--connect",
    "--play",
    "--file-transfer",
    "--view-camera",
    "--port-forward",
    "--terminal",
    "--rdp",
    "--switch_uuid",
];
const STAFF_ALLOWED: &[&str] = &[
    "--cm",
    "--whiteboard",
    "--check-hwcodec-config",
    "--version",
    "--build-date",
    "--get-id",
    "--uninstall-service",
];
const MANAGER_ALLOWED: &[&str] = &[
    "--connect",
    "--play",
    "--file-transfer",
    "--view-camera",
    "--port-forward",
    "--terminal",
    "--rdp",
    "--check-hwcodec-config",
    "--version",
    "--build-date",
    "--get-id",
];

/// Returns false when this variant must refuse the command line.
pub fn check_cli_args(args: &[String]) -> bool {
    // Staff and Production may never open an outgoing session, from any argument position.
    if is_receive_only() {
        let prefix = crate::get_uri_prefix().to_lowercase();
        for a in args {
            let is_outgoing_url = {
                let lower = a.to_lowercase();
                lower
                    .strip_prefix(prefix.as_str())
                    .map_or(false, |rest| !rest.chars().all(|c| c == '/'))
            };
            if OUTGOING_FLAGS.contains(&a.as_str()) || is_outgoing_url {
                eprintln!("Hope Desk receives connections only");
                log::warn!("Hope Desk receives connections only; refused {}", a);
                return false;
            }
        }
    }
    let Some(arg) = args.first() else {
        return true;
    };
    let allowed: Option<&[&str]> = match VARIANT {
        Variant::Staff => Some(STAFF_ALLOWED),
        Variant::Manager => Some(MANAGER_ALLOWED),
        Variant::Production => None,
    };
    if let Some(allowed) = allowed {
        if arg.starts_with("--") && !allowed.contains(&arg.as_str()) {
            eprintln!("{} is not available in {}", arg, DISPLAY_NAME);
            log::warn!("{} is not available in {}", arg, DISPLAY_NAME);
            return false;
        }
    }
    if !allows_service()
        && matches!(
            arg.as_str(),
            "--cm" | "--whiteboard" | "--check-hwcodec-config"
        )
    {
        start_orphan_watchdog();
        #[cfg(target_os = "macos")]
        if arg == "--cm" && is_receive_only() {
            crate::platform::macos::disable_app_nap();
        }
    }
    true
}

/// Called when the GUI starts with no arguments. Manager asks macOS to list it under Input
/// Monitoring (it grabs the keyboard when controlling a remote Mac).
pub fn on_gui_start() {
    #[cfg(target_os = "macos")]
    if is_receive_only() {
        crate::platform::macos::disable_app_nap();
    }
    #[cfg(target_os = "macos")]
    if HARD_SETTINGS
        .read()
        .unwrap()
        .get("hope-desk-needs-input-monitoring")
        .map_or(false, |v| v == "Y")
    {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            crate::platform::macos::request_input_monitoring_listing();
        });
    }
}

/// Logged once per incoming connection so a field report of "keys do nothing" shows which
/// permission or event source was missing.
#[cfg(target_os = "macos")]
pub fn log_input_injection_state(virtual_input_ok: bool) {
    log::info!(
        "Hope Desk input: virtual keyboard source {}, accessibility {}, input monitoring {}",
        if virtual_input_ok { "ok" } else { "FAILED" },
        crate::platform::macos::is_process_trusted(false),
        crate::platform::macos::is_can_input_monitoring(false),
    );
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

// ---------------------------------------------------------------------------------------------
// Change ID on our self-hosted (OSS) server.
//
// OSS hbbs answers the TCP RegisterPk-with-old_id that upstream's `check_id` sends with
// NOT_SUPPORT (changing an ID is a Pro feature). Its UDP RegisterPk, which the rendezvous mediator
// sends on every (re-)registration, ignores old_id: it accepts an ID nobody owns or one this
// machine (uuid) already owns, and answers UUID_MISMATCH when another machine owns it. Upstream
// reacts to UUID_MISMATCH with `Config::update_id()`, a RANDOM new ID, which would silently break
// a production machine's saved address. So for Hope Desk:
//   GUI process:    pre-check that nobody online holds the new ID, write it through the same IPC
//                   path upstream uses after a successful change, then wait for the server's verdict.
//   Server process: remember the previous ID on every IPC ID write and re-register at once; a
//                   UUID_MISMATCH for the new ID inside the window reverts to the previous ID.
// The old ID's record stays orphaned on the server (accepted).
// ---------------------------------------------------------------------------------------------

/// Shown by the Change ID dialog (Dart passes it through `translate`, which keeps unknown text).
pub const ID_IN_USE: &str = "That name is already in use";
pub const ID_NOT_CONFIRMED: &str =
    "The server didn't confirm the new ID. Check the network and try again in a minute.";
pub const ID_NOT_SAVED: &str = "Couldn't save the new ID";

#[cfg(not(target_os = "ios"))]
mod rename {
    use super::*;
    use std::{
        sync::Mutex,
        time::{Duration, Instant},
    };

    /// How long after a rename a UUID_MISMATCH for the new ID reverts it.
    const REVERT_WINDOW: Duration = Duration::from_secs(120);
    /// How long the GUI waits for the server to accept or reject the new ID.
    const CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);
    /// How long the GUI waits to see its IPC write applied at all.
    const APPLY_TIMEOUT: Duration = Duration::from_secs(5);

    struct PendingRename {
        old_id: String,
        new_id: String,
        at: Instant,
    }

    static PENDING_RENAME: Mutex<Option<PendingRename>> = Mutex::new(None);

    /// Server process (IPC "id" write, `ipc.rs`): the ID just changed from `old_id` to `new_id`.
    pub fn on_id_written(old_id: &str, new_id: &str) {
        if old_id.is_empty() || old_id == new_id {
            return;
        }
        *PENDING_RENAME.lock().unwrap() = Some(PendingRename {
            old_id: old_id.to_owned(),
            new_id: new_id.to_owned(),
            at: Instant::now(),
        });
        log::info!("Hope Desk: ID changed from {} to {}; re-registering", old_id, new_id);
        // Register the new ID now instead of on the next 15 s tick.
        crate::RendezvousMediator::restart();
    }

    /// Server process (`rendezvous_mediator.rs` UUID_MISMATCH): if `current_id` is a rename made
    /// inside the window, returns the ID to go back to (and forgets the rename).
    pub fn take_rename_to_revert(current_id: &str) -> Option<String> {
        let mut pending = PENDING_RENAME.lock().unwrap();
        let hit = matches!(
            pending.as_ref(),
            Some(p) if p.new_id == current_id && p.at.elapsed() < REVERT_WINDOW
        );
        let taken = pending.take();
        if hit {
            taken.map(|p| p.old_id)
        } else {
            None
        }
    }

    async fn ipc_id() -> Option<String> {
        let mut c = crate::ipc::connect(1000, "").await.ok()?;
        c.send(&crate::ipc::Data::Config(("id".to_owned(), None)))
            .await
            .ok()?;
        match c.next_timeout(1000).await {
            Ok(Some(crate::ipc::Data::Config((name, Some(v))))) if name == "id" => Some(v),
            _ => None,
        }
    }

    async fn ipc_key_confirmed() -> Option<bool> {
        let mut c = crate::ipc::connect(1000, "").await.ok()?;
        c.send(&crate::ipc::Data::OnlineStatus(None)).await.ok()?;
        match c.next_timeout(1000).await {
            Ok(Some(crate::ipc::Data::OnlineStatus(Some((_, confirmed))))) => Some(confirmed),
            _ => None,
        }
    }

    /// GUI process: change the ID when the server answered `server_not_support`.
    /// Returns "" on success, otherwise a message for the Change ID dialog.
    pub async fn change_id_without_server_support(new_id: &str, old_id: &str) -> &'static str {
        // (a) Pre-check. OSS hbbs answers OnlineRequest (port 21115) for any ID; a bit set means
        // a machine registered that ID within the last 30 s. Offline or unknown IDs are not
        // distinguishable here, so they fall through to the server's UUID check below.
        match crate::client::peer_online::query_online_states_(
            &vec![new_id.to_owned()],
            Duration::from_millis(3_000),
        )
        .await
        {
            Ok((onlines, _)) if onlines.iter().any(|x| x == new_id) => {
                log::info!("Hope Desk: {} is online elsewhere", new_id);
                return ID_IN_USE;
            }
            Ok(_) => {}
            Err(e) => log::warn!("Hope Desk: online pre-check for {} failed: {}", new_id, e),
        }

        // (b) Same IPC write upstream makes after a successful change. The server process sets
        // the ID, clears key_confirmed, records the rename and re-registers (c); config sync
        // carries it to the root service for the login window.
        if let Err(e) = crate::ipc::set_config_async("id", new_id.to_owned()).await {
            log::error!("Hope Desk: failed to write the new ID: {}", e);
            return ID_NOT_SAVED;
        }

        // (d)/(e) Wait for the verdict: confirmed under the new ID, or reverted by the server
        // process after UUID_MISMATCH.
        let start = Instant::now();
        let mut seen_new = false;
        while start.elapsed() < CONFIRM_TIMEOUT {
            hbb_common::sleep(0.5).await;
            let Some(id) = ipc_id().await else {
                continue;
            };
            if id == new_id {
                seen_new = true;
                if ipc_key_confirmed().await == Some(true) {
                    log::info!("Hope Desk: server accepted ID {}", new_id);
                    return "";
                }
            } else if id == old_id {
                if seen_new {
                    log::info!("Hope Desk: {} is owned by another computer; reverted", new_id);
                    return ID_IN_USE;
                }
                if start.elapsed() > APPLY_TIMEOUT {
                    return ID_NOT_SAVED;
                }
            } else {
                log::error!("Hope Desk: ID unexpectedly became {} during rename", id);
                return ID_NOT_CONFIRMED;
            }
        }
        // No answer from the server: go back to the old ID rather than leave a half-done rename.
        log::warn!("Hope Desk: no server verdict for {}; reverting to {}", new_id, old_id);
        crate::ipc::set_config_async("id", old_id.to_owned()).await.ok();
        ID_NOT_CONFIRMED
    }
}

#[cfg(not(target_os = "ios"))]
pub use rename::{change_id_without_server_support, on_id_written, take_rename_to_revert};
