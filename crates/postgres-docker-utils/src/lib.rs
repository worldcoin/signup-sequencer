use testcontainers::clients::Cli;
use testcontainers::{Container, RunnableImage};
use testcontainers_modules::postgres::Postgres;

pub struct DockerContainer<'a> {
    container: Container<'a, Postgres>,
}

impl<'a> DockerContainer<'a> {
    fn new(docker: &'a Cli) -> Self {
        let image =
            RunnableImage::from(Postgres::default().with_host_auth()).with_tag("16.2-alpine");
        let container = docker.run(image);
        DockerContainer { container }
    }

    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.container.get_host_port_ipv4(5432))
    }
}

pub async fn setup(docker: &Cli) -> anyhow::Result<DockerContainer> {
    Ok(DockerContainer::new(docker))
}
