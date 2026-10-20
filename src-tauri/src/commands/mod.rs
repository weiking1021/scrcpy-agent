//! The commands the UI can call, grouped by what they are about. A command
//! turns its arguments into a call to a core service and its answer into what
//! the UI gets; what is decided is decided in the core.
//!
//! Commands return the typed `CoreError`; the UI switches on its `code`.

pub mod addons;
pub mod apps;
pub mod console;
pub mod devices;
pub mod files;
pub mod requests;
pub mod scrcpy;
pub mod sessions;
pub mod settings;
pub mod shortcuts;
pub mod themes;
