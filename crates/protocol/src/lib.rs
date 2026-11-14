pub const PROTOCOL_VERSION: u32 = 1;

/// Lines of history a session's terminal keeps, in the daemon's model and in
/// the app's grid alike: a pane that reattaches is painted from the daemon's
/// history, so both hold the same amount. One line costs about 24 bytes per
/// column, so 5,000 lines of a 120-column terminal is about 14 MB.
pub const SCROLLBACK_LINES: usize = 5_000;

pub mod frame;
pub mod message;
pub mod session;
