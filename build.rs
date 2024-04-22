use std::process::Command;

fn main() {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .expect("Failed to execute git command");

    let git_description = String::from_utf8_lossy(&output.stdout).trim().to_string();

    println!("cargo:rustc-env=GIT_VERSION={}", git_description);
}
