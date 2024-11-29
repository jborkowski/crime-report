fn main() {
    // Set the package version from the latest git tag
    println!(
        "cargo:rustc-env=CARGO_PKG_VERSION={}",
        git_version::git_version!()
    );
}
