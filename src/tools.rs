//! See openspec/changes/carry-v0-1/design.md for this module's surface.
#![allow(dead_code, unused)]

/// Entry point when carry runs under a tool's name. Returns an exit code only on failure.
pub fn launch_from_argv0(name: &str) -> i32 {
    eprintln!("carry: tool launcher for '{name}' not implemented yet");
    127
}
