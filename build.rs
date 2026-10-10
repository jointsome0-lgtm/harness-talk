//! Says what this target has, so the program asks for a part and never for a system. A port
//! adds its system to a line here; until then the part answers `unsupported_on_this_platform`.
use std::env;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for part in ["native_clients", "codex_command", "mcp_server", "catalog"] {
        println!("cargo::rustc-check-cfg=cfg({part})");
    }
    let system = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let linux = system == "linux";
    // Notices into Codex and Claude Code by every route, their discovery, and `receive`.
    if linux || system == "macos" {
        println!("cargo::rustc-cfg=native_clients");
    }
    // A notice into Codex through its installed command, where nothing else of the two
    // clients is ported.
    if linux || system == "macos" || system == "windows" {
        println!("cargo::rustc-cfg=codex_command");
    }
    // `htalk mcp`: the server runs every call as a child whose process group it owns.
    if linux || system == "macos" || system == "windows" {
        println!("cargo::rustc-cfg=mcp_server");
    }
    // The catalogue of profiles, where the `catalog` feature asks for it.
    if linux && env::var_os("CARGO_FEATURE_CATALOG").is_some() {
        println!("cargo::rustc-cfg=catalog");
    }
}
