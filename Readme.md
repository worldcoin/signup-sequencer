# Rust project template

![lines of code](https://img.shields.io/tokei/lines/github/recmo/rust-app-template)
[![dependency status](https://deps.rs/repo/github/recmo/rust-app-template/status.svg)](https://deps.rs/repo/github/recmo/rust-app-template)
[![codecov](https://img.shields.io/codecov/c/github/recmo/rust-app-template)](https://codecov.io/gh/Recmo/rust-app-template)
[![build](https://img.shields.io/github/workflow/status/recmo/rust-app-template/build)](https://github.com/Recmo/rust-app-template/actions?query=workflow%3Abuild)
[![deploy-gke](https://img.shields.io/github/workflow/status/recmo/rust-app-template/deploy-gke)](https://github.com/Recmo/rust-app-template/actions?query=workflow%3Adeploy-gke)


*Template to run a Rust executable in Google Kubernetes Engine using Github Actions for deployment.*

**Note.** The application will be statically linked with [musl](https://musl.libc.org/) and deployed
in an otherwise empty container. Crates that dynamically link (for example to OpenSSL) will not work
without modifying the Dockerfile.

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

## To do

* Add badges to readme
* Add tests and run in CI
* Add criterion benchmarks and run in CI
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
