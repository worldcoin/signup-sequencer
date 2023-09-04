use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Context;

pub struct DockerContainerGuard {
    container_id:   String,
    container_port: u16,
}

impl DockerContainerGuard {
    pub fn port(&self) -> u16 {
        self.container_port
    }
}

impl Drop for DockerContainerGuard {
    fn drop(&mut self) {
        if let Err(err) = run_cmd(&format!("docker stop {}", &self.container_id)) {
            eprintln!("Failed to stop docker container: {}", err);
        }

        // Redundant, but better safe than sorry
        if let Err(err) = run_cmd(&format!("docker rm {}", &self.container_id)) {
            eprintln!("Failed to remove docker container: {}", err);
        }
    }
}

/// Starts a postgres docker container that will accept all connections with a
/// random port assigned by the os or docker. The container will be stopped and
/// removed when the guard is dropped.
///
/// Note that we're using sync code here so we'll block the executor - but this
/// is fine, because the spawned container will still run in the background.
pub async fn setup() -> anyhow::Result<DockerContainerGuard> {
    let container_id =
        run_cmd_to_output("docker run --rm -d -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432 postgres")
            .context("Starting the Postgres container")?;

    let exposed_port = run_cmd_to_output(&format!("docker container port {container_id} 5432"))
        .context("Fetching container exposed port")?;
    let container_port = parse_exposed_port(&exposed_port)?;

    std::thread::sleep(Duration::from_secs_f32(2.0));

    Ok(DockerContainerGuard {
        container_id,
        container_port,
    })
}

fn run_cmd_to_output(cmd_str: &str) -> anyhow::Result<String> {
    let args: Vec<_> = cmd_str.split(' ').collect();
    let mut command = Command::new(args[0]);

    for arg in &args[1..] {
        command.arg(arg);
    }

    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());

    let Ok(output) = command.output() else {
        return Ok(String::new());
    };

    let utf = String::from_utf8(output.stdout)?;

    Ok(utf.trim().to_string())
}

fn run_cmd(cmd_str: &str) -> anyhow::Result<()> {
    run_cmd_to_output(cmd_str)?;

    Ok(())
}

fn parse_exposed_port(s: &str) -> anyhow::Result<u16> {
    let parts: Vec<_> = s
        .split_whitespace()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    let ports: Vec<_> = parts.into_iter().filter_map(extract_port).collect();

    let mut parsed_port = None;

    for port in ports {
        let port: u16 = port.parse().with_context(|| format!("Parsing `{port}`"))?;

        if let Some(current) = parsed_port {
            if current != port {
                anyhow::bail!(
                    "Multiple different ports exposed: `{}` and `{}`",
                    current,
                    port
                );
            }
        } else {
            parsed_port = Some(port);
        }
    }

    parsed_port.context("No ports parsed")
}

fn extract_port(s: &str) -> Option<&str> {
    s.split(':').last()
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("0.0.0.0:55837" => 55837 ; "base case")]
    #[test_case("   0.0.0.0:55837    " => 55837 ; "ignore whitespace")]
    #[test_case("[::]:12345" => 12345 ; "works with ipv6")]
    #[test_case("0.0.0.0:12345 \n [::]:12345" => 12345 ; "works with multiple ips")]
    fn test_parse_exposed_port(s: &str) -> u16 {
        parse_exposed_port(s).unwrap()
    }

    #[test]
    fn different_ports_result_in_failure() {
        const S: &str = "0.0.0.0:12345 [::]:54321";

        let _err = parse_exposed_port(S).unwrap_err();
    }
}
