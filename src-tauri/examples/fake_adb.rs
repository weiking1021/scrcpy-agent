//! A stand-in for the adb program, used by the integration tests (never
//! shipped).
//!
//! The application only runs the adb program to start the adb server; all
//! else it says to the server itself (the tests run a fake server, see
//! `tests/app_flow.rs`). So this program accepts `start-server` and nothing
//! else, and exits with an error for any other call: if the application
//! starts to use the adb command line again, the tests fail. Every call is
//! appended to `calls.log` (one JSON array of arguments per line).

use std::io::Write;
use std::path::PathBuf;
use std::{env, fs, process};

fn main() {
    let dir: PathBuf = env::current_exe()
        .expect("own path")
        .parent()
        .expect("parent dir")
        .to_path_buf();
    let args: Vec<String> = env::args().skip(1).collect();

    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("calls.log"))
        .expect("open calls.log");
    // One call per line as a JSON array, so argument boundaries survive.
    writeln!(log, "{}", serde_json::to_string(&args).expect("encode the call")).expect("write calls.log");

    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["start-server"] => {}
        other => {
            eprintln!("fake adb: unsupported arguments {other:?}");
            process::exit(2);
        }
    }
}
