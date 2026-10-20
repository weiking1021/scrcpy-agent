//! A stand-in for the adb program that answers from a script, used to test
//! `CliAdb` (never shipped).
//!
//! It reads `script.json` next to the executable: a list of
//! `{"args": [...], "stdout": "...", "stderr": "...", "exit": N}`. A call whose
//! arguments match an entry exactly gets that answer; any other call exits
//! with 99. Every call is appended to `calls.log` (one JSON array per line).

use serde_json::Value;
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
    writeln!(log, "{}", serde_json::to_string(&args).expect("encode the call")).expect("write calls.log");

    let script: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("script.json")).unwrap_or_else(|_| "[]".into()))
            .expect("script.json is a list");
    let wanted = serde_json::to_value(&args).expect("args as json");
    let Some(entry) = script.iter().find(|e| e["args"] == wanted) else {
        eprintln!("fake adb: no scripted answer for {args:?}");
        process::exit(99);
    };
    let text = |key: &str| entry[key].as_str().unwrap_or("").as_bytes().to_vec();
    std::io::stdout().write_all(&text("stdout")).expect("write stdout");
    std::io::stderr().write_all(&text("stderr")).expect("write stderr");
    process::exit(entry["exit"].as_i64().unwrap_or(0) as i32);
}
