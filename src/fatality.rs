//! Fatal-error reporting.
//!
//! Port of `Marksman.Fatality`. A failed state mutation is unrecoverable: the
//! server's view of the workspace can no longer be trusted, so the process
//! reports enough context to diagnose the failure and exits.

use std::process::exit;

use crate::misc;
use crate::state::State;

fn client_debug_out(info: &lsp_types::ClientInfo) {
    eprint!("Client: {}", info.name);
    if let Some(version) = &info.version {
        eprint!("{version}");
    }
    eprintln!();
}

fn state_debug_out(state: &State) {
    if let Some(info) = &state.client().info {
        client_debug_out(info);
    }

    let workspace = state.workspace();
    let user_config_string = if workspace.user_config().is_some() { "present" } else { "absent" };

    eprintln!("Workspace:");
    eprintln!("  Revision      : {}", state.revision());
    eprintln!("  Folder count  : {}", workspace.folder_count());
    eprintln!("  Document count: {}", workspace.doc_count());
    eprintln!("  User config   : {user_config_string}");
}

/// Prints a crash report to stderr and terminates the process.
pub fn abort(state_opt: Option<&State>, message: &str) -> ! {
    const SEPARATOR: &str =
        "---------------------------------------------------------------------------";

    eprintln!("{SEPARATOR}");
    eprintln!("Marksman encountered a fatal error");
    eprintln!("Please, report the error at https://github.com/artempyanykh/marksman/issues");
    eprintln!("{SEPARATOR}");
    eprintln!("Marksman version: {}", env!("CARGO_PKG_VERSION"));
    eprintln!("OS: {}", std::env::consts::OS);
    eprintln!("Arch: {}", std::env::consts::ARCH);

    if let Some(state) = state_opt {
        state_debug_out(state);
    }

    eprintln!("{SEPARATOR}");
    for line in misc::lines_of(message) {
        eprintln!("{line}");
    }

    exit(1)
}
