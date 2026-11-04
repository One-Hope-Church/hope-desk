use librustdesk::*;

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    crate::common::load_custom_client();
    if !librustdesk::hope_desk::allows_service() {
        eprintln!("The service is not available in Hope Desk");
        std::process::exit(1);
    }
    if args.len() > 1 && args[1] == "--write-plists" {
        if let Err(e) = librustdesk::platform::write_plists() {
            eprintln!("Failed to write plists: {}", e);
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    hbb_common::init_log(false, "service");
    crate::start_os_service();
}
