//! Library crate for the `gokod` daemon.

pub mod auth;
pub mod buffer;
pub mod lock;
pub mod paths;
pub mod pty;
pub mod server;

#[cfg(test)]
mod test_support;
