//! Stamps the binary with what it was built from, for About Ayeaye: the
//! commit (with `-dirty` when the tree had uncommitted changes) and when.

use std::process::Command;

fn main() {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
    let build_id = if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    };
    let built_at = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();

    println!("cargo:rustc-env=AYEAYE_BUILD_ID={build_id}");
    println!("cargo:rustc-env=AYEAYE_BUILT_AT={built_at}");
    // A commit, a checkout or an edit is a new build; nothing else is.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
}
