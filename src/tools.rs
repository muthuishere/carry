//! See openspec/changes/carry-v0-1/design.md for this module's surface.
#![allow(dead_code, unused)]

/// Entry point when carry runs under a tool's name. Returns an exit code only on failure.
pub fn launch_from_argv0(name: &str) -> i32 {
    eprintln!("carry: tool launcher for '{name}' not implemented yet");
    127
}

/// Link every app as `<root>/bin/<name>` -> the carry executable (busybox-style).
pub fn link_tools(
    root: &std::path::Path,
    apps: &std::collections::BTreeMap<String, crate::model::ToolRef>,
) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin)?;
    for name in apps.keys() {
        let tmp = bin.join(format!(".{name}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        std::os::unix::fs::symlink(&exe, &tmp)?;
        std::fs::rename(&tmp, bin.join(name))?;
    }
    Ok(())
}
