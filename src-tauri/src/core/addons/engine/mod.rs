//! The engine that runs the `main.js` of an add-on.
//!
//! Each add-on runs alone, in a thread and a script runtime of its own, with a limit of memory and of
//! time. The script has no files, no network and no programs: only what `host` gives it, and what
//! needs a permission is refused without it (`Granted::check`). What the script asks of the program
//! goes through `AddonHost`, so the engine is the same on every system and is tested with a fake host.

pub mod host;
pub mod limits;
pub mod runtime;
pub mod timers;
