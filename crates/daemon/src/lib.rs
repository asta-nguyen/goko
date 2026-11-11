//! Library crate for the `gokod` daemon.

pub mod auth;
pub mod buffer;
pub mod lock;
pub mod paths;
pub mod pty;
pub mod server;
pub mod session;
pub mod terminal;

#[cfg(test)]
mod test_support;
