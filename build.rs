use anyhow::{anyhow, Context, Error, Result};
use chrono::Utc;
use std::{
    env::{var, VarError},
    fs,
    path::Path,
    process::Command,
};

fn main() -> Result<()> {
    rerun_if_git_changes()?;

    println!(
        "cargo:rustc-env=COMMIT_SHA={}",
        env_or_cmd("COMMIT_SHA", &["git", "rev-parse", "HEAD"])?
    );
    println!(
        "cargo:rustc-env=COMMIT_DATE={}",
        env_or_cmd("COMMIT_DATE", &[
            "git",
            "log",
            "-n1",
            "--pretty=format:'%ad'",
            "--date=short"
        ])?
        .trim_matches('\'')
    );
    println!("cargo:rustc-env=BUILD_DATE={}", Utc::today().naive_utc());
    println!(
        "cargo:rustc-env=TARGET={}",
        var("TARGET").context("Fetching environment variable TARGET")?
    );
    Ok(())
}

fn env_or_cmd(env: &str, cmd: &[&str]) -> Result<String> {
    // Try env first
    match var(env) {
        Ok(s) => return Ok(s),
        Err(VarError::NotPresent) => (),
        Err(e) => return Err(Error::new(e)),
    };

    // Try command
    let output = Command::new(cmd[0]).args(&cmd[1..]).output()?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    } else {
        Err(anyhow!(
            "Variable {} is unset and command \"{}\" failed",
            env,
            cmd.join(" ")
        ))
    }
}

fn rerun_if_git_changes() -> Result<()> {
    // Skip if not in a git repo
    if !Path::new(".git/HEAD").exists() {
        eprintln!("No .git/HEAD found, not rerunning on git change");
        return Ok(());
    }

    // TODO: Worktree support where `.git` is a file
    println!("cargo:rerun-if-changed=.git/HEAD");

    // Determine where HEAD points and echo that path also.
    let contents = String::from_utf8(fs::read(".git/HEAD")?)?;
    let head_ref = contents.split(": ").collect::<Vec<_>>();
    if head_ref.len() == 2 && head_ref[0] == "ref" {
        println!("cargo:rerun-if-changed=.git/{}", head_ref[1]);
        Ok(())
    } else {
        Err(anyhow!("Can not parse .git/HEAD"))
    }
}
