use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Error};
use hyper::{Body, Client};
use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng};
use tracing::{debug, info};
use tracing_subscriber::fmt::format;

use crate::common::prelude::{Request, StatusCode};

const LOCAL_ADDR: &str = "localhost";

#[derive(Debug)]
pub struct DockerComposeGuard<'a> {
    // Current working dir containing compose.yml
    cwd: &'a str,
    project_name: String,
    chain_port: u32,
    tx_sitter_db_port: u32,
    sequencer_db_port: u32,
    tx_sitter_port: u32,
    semaphore_insertion_port: u32,
    semaphore_deletion_port: u32,
    signup_sequencer_0_port: u32,
    signup_sequencer_1_port: u32,
    signup_sequencer_2_port: u32,
    signup_sequencer_3_port: u32,
    signup_sequencer_balancer_port: u32,
}

impl DockerComposeGuard<'_> {
    pub fn get_local_addr(&self) -> String {
        format!("{}:{}", LOCAL_ADDR, self.signup_sequencer_balancer_port)
    }

    pub fn get_chain_addr(&self) -> String {
        format!("{}:{}", LOCAL_ADDR, self.chain_port)
    }

    pub async fn restart_sequencer(&self) -> anyhow::Result<()> {
        let (stdout, stderr) = run_cmd_to_output(
            self.cwd,
            self.envs_with_ports(),
            self.generate_command("restart signup-sequencer-0"),
        )
        .context("Restarting sequencer.")?;

        debug!(
            "Docker compose rstart output:\n stdout:\n{}\nstderr:\n{}\n",
            stdout, stderr
        );

        tokio::time::sleep(Duration::from_secs(5)).await;

        await_running(self).await
    }

    fn envs_with_ports(&self) -> HashMap<String, String> {
        let mut res = HashMap::new();

        res.insert(String::from("CHAIN_PORT"), self.chain_port.to_string());
        res.insert(
            String::from("TX_SITTER_DB_PORT"),
            self.tx_sitter_db_port.to_string(),
        );
        res.insert(
            String::from("SEQUENCER_DB_PORT"),
            self.sequencer_db_port.to_string(),
        );
        res.insert(
            String::from("TX_SITTER_PORT"),
            self.tx_sitter_port.to_string(),
        );
        res.insert(
            String::from("SEMAPHORE_INSERTION_PORT"),
            self.semaphore_insertion_port.to_string(),
        );
        res.insert(
            String::from("SEMAPHORE_DELETION_PORT"),
            self.semaphore_deletion_port.to_string(),
        );
        res.insert(
            String::from("SIGNUP_SEQUENCER_0_PORT"),
            self.signup_sequencer_0_port.to_string(),
        );
        res.insert(
            String::from("SIGNUP_SEQUENCER_1_PORT"),
            self.signup_sequencer_1_port.to_string(),
        );
        res.insert(
            String::from("SIGNUP_SEQUENCER_2_PORT"),
            self.signup_sequencer_2_port.to_string(),
        );
        res.insert(
            String::from("SIGNUP_SEQUENCER_3_PORT"),
            self.signup_sequencer_3_port.to_string(),
        );
        res.insert(
            String::from("SIGNUP_SEQUENCER_BALANCER_PORT"),
            self.signup_sequencer_balancer_port.to_string(),
        );

        res
    }

    fn generate_command(&self, args: &str) -> String {
        format!(
            "docker compose -p {} --profile e2e-ha {}",
            self.project_name, args
        )
    }

    fn update_balancer_port(&mut self, signup_sequencer_balancer_port: u32) {
        self.signup_sequencer_balancer_port = signup_sequencer_balancer_port
    }

    fn update_chain_port(&mut self, chain_port: u32) {
        self.chain_port = chain_port
    }

    fn get_mapped_port(&self, service_name: &str, port: u32) -> anyhow::Result<u32> {
        let (stdout, stderr) = run_cmd_to_output(
            self.cwd,
            self.envs_with_ports(),
            self.generate_command(format!("port {service_name} {port}").as_str()),
        )
        .context("Looking for balancer selected port.")?;

        debug!(
            "Docker compose starting output:\n stdout:\n{}\nstderr:\n{}\n",
            stdout, stderr
        );

        parse_exposed_port(stdout)
    }
}

impl Drop for DockerComposeGuard<'_> {
    fn drop(&mut self) {
        // May run when compose is not up but better to be sure its down.
        // Parameter '-v' is removing all volumes and networks.
        if let Err(err) = run_cmd(
            self.cwd,
            self.envs_with_ports(),
            self.generate_command("down -v"),
        ) {
            eprintln!("Failed to put down docker compose: {}", err);
        }
    }
}

/// Starts a docker compose infrastructure. It will be stopped and removed when
/// the guard is dropped.
///
/// Note that we're using sync code here so we'll block the executor - but this
/// is fine, because the spawned container will still run in the background.
pub async fn setup(cwd: &str, offchain_mode: bool) -> anyhow::Result<DockerComposeGuard> {
    let mut res = DockerComposeGuard {
        cwd,
        project_name: generate_project_name(),
        chain_port: 0,
        tx_sitter_db_port: 0,
        sequencer_db_port: 0,
        tx_sitter_port: 0,
        semaphore_insertion_port: 0,
        semaphore_deletion_port: 0,
        signup_sequencer_0_port: 0,
        signup_sequencer_1_port: 0,
        signup_sequencer_2_port: 0,
        signup_sequencer_3_port: 0,
        signup_sequencer_balancer_port: 0,
    };

    debug!("Configuration: {:#?}", res);

    let (stdout, stderr) = run_cmd_to_output(
        res.cwd,
        res.envs_with_ports(),
        res.generate_command("up -d"),
    )
    .context("Starting e2e test docker compose infrastructure.")?;

    debug!(
        "Docker compose starting output:\n stdout:\n{}\nstderr:\n{}\n",
        stdout, stderr
    );

    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut balancer_port = Err(anyhow!("Balancer port not queried."));
    for _ in 0..3 {
        balancer_port = res.get_mapped_port("signup-sequencer-balancer", 8080);
        if balancer_port.is_ok() {
            break;
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    res.update_balancer_port(balancer_port?);

    if !offchain_mode {
        let mut chain_port = Err(anyhow!("Chain port not queried."));
        for _ in 0..3 {
            chain_port = res.get_mapped_port("chain", 8545);
            if chain_port.is_ok() {
                break;
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        res.update_chain_port(chain_port?);
    }

    await_running(&res).await?;

    Ok(res)
}

fn generate_project_name() -> String {
    thread_rng()
        .sample_iter(Alphanumeric)
        .filter(|c| c.is_ascii_lowercase())
        .take(8)
        .map(char::from)
        .collect()
}

async fn await_running(docker_compose_guard: &DockerComposeGuard<'_>) -> anyhow::Result<()> {
    let timeout = Duration::from_secs_f32(600.0);
    let check_interval = Duration::from_secs_f32(1.0);
    let min_success_counts = 5;
    let mut success_counter = 0;

    let timer = Instant::now();
    loop {
        let healthy = check_health(docker_compose_guard.get_local_addr()).await;
        if healthy.is_ok() && healthy.unwrap() {
            success_counter += 1;
            info!("Health check passed (success_counter={})", success_counter);
        }

        if success_counter >= min_success_counts {
            return Ok(());
        }

        if timer.elapsed() > timeout {
            return Err(Error::msg("Timed out waiting for healthcheck."));
        }

        tokio::time::sleep(check_interval).await;
    }
}

async fn check_health(local_addr: String) -> anyhow::Result<bool> {
    let uri = format!("http://{}", local_addr);
    let client = Client::new();

    let healthcheck = Request::builder()
        .method("GET")
        .uri(format!("{uri}/health"))
        .header("Content-Type", "application/json")
        .body(Body::empty())
        .unwrap();

    let response = client.request(healthcheck).await?;

    Ok(response.status() == StatusCode::OK)
}

fn run_cmd_to_output(
    cwd: &str,
    envs: HashMap<String, String>,
    cmd_str: String,
) -> anyhow::Result<(String, String)> {
    let args: Vec<_> = cmd_str.split(' ').collect();
    let mut command = Command::new(args[0]);

    for arg in &args[1..] {
        command.arg(arg);
    }

    command
        .current_dir(cwd)
        .envs(envs)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = command
        .output()
        .with_context(|| format!("Failed to run command: {}", cmd_str))?;

    let stdout_utf = String::from_utf8(output.stdout)?;
    let stderr_utf = String::from_utf8(output.stderr)?;

    Ok((stdout_utf.trim().to_string(), stderr_utf.trim().to_string()))
}

fn run_cmd(cwd: &str, envs: HashMap<String, String>, cmd_str: String) -> anyhow::Result<()> {
    run_cmd_to_output(cwd, envs, cmd_str)?;

    Ok(())
}

fn parse_exposed_port(s: String) -> anyhow::Result<u32> {
    let parts: Vec<_> = s
        .split_whitespace()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let port = parts.last().and_then(|v| v.split(':').next_back());

    match port {
        Some(port) => port.parse::<u32>().map_err(|err| anyhow!(err)),
        None => Err(anyhow!("Port not found in string.")),
    }
}
