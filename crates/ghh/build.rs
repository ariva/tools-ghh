use std::process::Command;

fn command_output(command: &str, arguments: &[&str]) -> Option<String> {
    Command::new(command)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().to_owned())
        .filter(|output| !output.is_empty())
}

fn main() {
    // The crate lives under crates/, so ask git where the repository is
    // instead of assuming `.git` next to this file.
    if let Some(git_dir) = command_output("git", &["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }

    let build_hash = command_output("git", &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|| "unknown".to_owned());
    let build_date = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|epoch| {
            command_output(
                "date",
                &["-u", "-d", &format!("@{epoch}"), "+%Y-%m-%dT%H:%M:%SZ"],
            )
        })
        .or_else(|| command_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]))
        .unwrap_or_else(|| "unknown".to_owned());

    println!("cargo:rustc-env=GIT_HELPER_BUILD_HASH={build_hash}");
    println!("cargo:rustc-env=GIT_HELPER_BUILD_DATE={build_date}");
}
