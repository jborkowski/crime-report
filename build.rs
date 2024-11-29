use std::process::Command;
fn main() {
    let git_version = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    println!("cargo:rustc-env=CARGO_PKG_VERSION={}", git_version);
}
