//! Compatibility for the old Semaphore and optional SVM downloaders' HTTP APIs.
//! Re-export maintained Reqwest; no Reqwest 0.11 implementation or Hyper 0.14
//! code is included. Remove when both downloaders use maintained HTTP clients.
pub use reqwest_current::*;
