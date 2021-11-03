use anyhow::{anyhow, Context, Error, Result};
use chrono::Utc;
use ethers::{prelude::Abigen, utils::Solc};
use std::{env::{var, VarError}, fs::{self, File}, io::Write, path::{Path}, process::Command};

const WALLET_CLAIMS_PATH: &str = "solidity/contracts/WalletClaims.sol";

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

    build_contracts_abi();

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

fn build_contracts_abi() {
    println!("cargo:rerun-if-changed={}", WALLET_CLAIMS_PATH);

    let contracts = Solc::new_with_paths(vec![WALLET_CLAIMS_PATH.to_string(), "@openzeppelin/=solidity/node_modules/@openzeppelin/".to_string()])//, "solidity/hubble-contracts/contracts".to_string()])
        .arg("--allow-paths=.,").build_raw().expect("Could not compile");
    println!("Contracts {:?}", contracts);

    let contract = contracts.get("WalletClaims").expect("contract not found");

    let abi = contract.abi.clone();

    let mut f = File::create("walletclaims.bin").expect("could not create WalletClaims bytecode file");
    f.write_all(contract.bin.as_bytes())
        .expect("could not write WalletClaims bytecode to the file");

    // generate type-safe bindings to it
    // TODO this currently generates bad rust code because of some shoddy formatting -- needs to be fixed.
    let bindings = Abigen::new("WalletClaims", abi)
        .expect("could not instantiate Abigen")
        .generate()
        .expect("could not generate bindings");
    bindings
        .write_to_file("./src/walletclaims_contract.rs")
        .expect("could not write bindings to file");
}