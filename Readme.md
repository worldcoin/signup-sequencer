# Rust project template

![lines of code](https://img.shields.io/tokei/lines/github/recmo/rust-app-template)
[![dependency status](https://deps.rs/repo/github/recmo/rust-app-template/status.svg)](https://deps.rs/repo/github/recmo/rust-app-template)
[![codecov](https://img.shields.io/codecov/c/github/recmo/rust-app-template)](https://codecov.io/gh/Recmo/rust-app-template)
[![build](https://img.shields.io/github/workflow/status/recmo/rust-app-template/build)](https://github.com/Recmo/rust-app-template/actions?query=workflow%3Abuild)
[![deploy-gke](https://img.shields.io/github/workflow/status/recmo/rust-app-template/deploy-gke)](https://github.com/Recmo/rust-app-template/actions?query=workflow%3Adeploy-gke)

**Main features.** Comes with the kitchen sink. Remove what you don't need.

* Command line argument parsing using `StructOpt`.
* Version info including commit hash.
* Error handling using `anyhow` and `thiserror`.
* Logging using `tracing` with `log` and `futures` compatibility, `-v`, `-vv`, etc. command line arguments.
* Preloaded with `serde`, `rand`, `rayon`, `itertools`.
* Tests using `proptest`, `pretty_assertions` and `float_eq`. I recommend the [closure style](https://docs.rs/proptest/0.10.1/proptest/macro.proptest.html#closure-style-invocation) proptests.
* Benchmarks using `criterion` (run `cargo criterion`).
* Dependencies build optimized, also in dev build.
* From scratch Docker build statically linked to musl.
* Github actions for build, test, linting and deployment to Google Kubernetes Engine.
## Setup

The Google Cloud project is taken from the Github Actions secret `GKE_PROJECT`.

The Github Actions secrete `GKE_SA_KEY` should contain the JSON key for a GCP `serviceAccount` with sufficient permissions.

Edit `.github/workflows/google.yml` and change `GKE_ZONE`, `GKE_CLUSTER`, `IMAGE`, and `DEPLOYMENT_NAME`.

## Tricks

Run the latest container locally

```
docker pull gcr.io/two-pi-com/rust-app-template-image:latest
docker run --rm -ti -p 8080:8080 gcr.io/two-pi-com/rust-app-template-image:latest version
```

Combining Tokio and Rayon:

<https://www.reddit.com/r/rust/comments/gwm84y/how_can_i_mix_rayon_and_tokio_properly/>

## To do

* Code coverage in CI
* Build cache in CI
* no_std support, CI test using no-std target.
* To do scraper in CI.
* `--threads` cli argument for Rayon worker pool size.
* `--trace` cli argument for `tracing-chrome`.
* `--seed` cli argument for deterministic `rand`.
* Rustdocs with Katex.
* Long running / fuzz mode for proptests.
* [`loom`](https://crates.io/crates/loom) support for concurrency testing, maybe [`simulation`](https://github.com/tokio-rs/simulation).
* Run benchmarks in CI on dedicated machine.
* Generate documentation in CI
* Add code coverage to CI
* Add license, contributing, and other changelogs
* Add ISSUE_TEMPLATE, PR template, etc.
* Add crates.io publishing
* Build ARM image

## References

* Deploying a container on GKE using the CLI.
  <https://cloud.google.com/kubernetes-engine/docs/tutorials/hello-app>
* Deploying an app to GKE using config files.
  <https://cloud.google.com/kubernetes-engine/docs/quickstarts/deploying-a-language-specific-app>
* Kubernetes documentation for `deployment.yaml`.
  <https://kubernetes.io/docs/concepts/workloads/controllers/deployment/>
* Example workflow for Github Acions and Google Kubernetes Engine
  <https://github.com/google-github-actions/setup-gcloud/tree/master/example-workflows/gke>
* Github Actions Environment Variables
  <https://docs.github.com/en/free-pro-team@latest/actions/reference/environment-variables#default-environment-variables>
* GitHub Actions context variables and expressions
  <https://docs.github.com/en/free-pro-team@latest/actions/reference/context-and-expression-syntax-for-github-actions>
* GitHub Actions workflow commands
  <https://docs.github.com/en/free-pro-team@latest/actions/reference/workflow-commands-for-github-actions>
