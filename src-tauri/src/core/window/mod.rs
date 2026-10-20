//! Windows on this computer: where they go, and the one that carries a
//! device's screen.

pub mod free;
pub mod layout;
pub mod locator;
#[cfg(windows)]
pub mod native;
pub mod toolbar;
