mod cli;
mod crypto;
mod ctx;
mod error;
mod model;
mod ops;
mod repo;
mod restore;
mod run;
mod settings;
mod store;
mod tools;

fn main() {
    // Invoked through a tool symlink (busybox-style): dispatch to the launcher.
    let argv0 = std::env::args().next().unwrap_or_default();
    let name = std::path::Path::new(&argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("carry")
        .to_string();
    if name != "carry" && !name.starts_with("carry-") && !name.starts_with("carry.") {
        std::process::exit(tools::launch_from_argv0(&name));
    }
    std::process::exit(cli::main());
}
