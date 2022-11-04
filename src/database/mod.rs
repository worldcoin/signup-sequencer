#[cfg(feature = "unstable_db")]
mod concrete_database;

#[cfg(feature = "unstable_db")]
pub use concrete_database::*;

#[cfg(not(feature = "unstable_db"))]
mod null_database;

#[cfg(not(feature = "unstable_db"))]
pub use null_database::*;