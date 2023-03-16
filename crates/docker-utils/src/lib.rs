use std::{
    process::{Command, Stdio},
    time::Duration,
};

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
        run_cmd(&format!("docker stop {}", &self.container_id));

        // Redundant, but better safe than sorry
        run_cmd(&format!("docker rm {}", &self.container_id));
    }
}

/// Starts a postgres docker container that will accept all connections with a
/// random port assigned by the os or docker. The container will be stopped and
/// removed when the guard is dropped.
///
/// Note that we're using sync code here so we'll block the executor - but this
/// is fine, because the spawned container will still run in the background.
pub async fn setup() -> anyhow::Result<DockerContainerGuard> {
    let container_id = run_cmd_to_output(&format!(
        "docker run --rm -d -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432 postgres",
    ));

    let exposed_port = run_cmd_to_output(&format!("docker container port {container_id} 5432"));
    let container_port = parse_exposed_port(&exposed_port);

    std::thread::sleep(Duration::from_secs_f32(1.0));

    Ok(DockerContainerGuard {
        container_id,
        container_port,
    })
}

fn run_cmd_to_output(cmd_str: &str) -> String {
    let args: Vec<_> = cmd_str.split(" ").collect();
    let mut command = Command::new(args[0]);

    for arg in &args[1..] {
        command.arg(arg);
    }

    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());

    let Ok(output) = command.output() else {
        return String::new();
    };

    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn run_cmd(cmd_str: &str) {
    run_cmd_to_output(cmd_str);
}

fn parse_exposed_port(s: &str) -> u16 {
    let parts: Vec<_> = s.split(":").collect();
    parts[1].parse().unwrap()
}
