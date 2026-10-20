//! A stand-in for the programs that add-ons start, used to test `ProcessTable` (never shipped).
//!
//! The first argument is what it does:
//! * `lines <text>...`: writes each argument as a line, then ends with code 0.
//! * `exit <code>`: ends with that code at once.
//! * `hang`: does nothing until it is ended.
//! * `sum`: reads its input to the end, then writes `sum <bytes> <wrapping sum of the bytes>` and ends.
//! * `long <n>`: writes a line of `n` characters, then the line `after`.

use std::io::{Read, Write};
use std::time::Duration;
use std::{env, process};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut out = std::io::stdout();
    match args.first().map(String::as_str) {
        Some("lines") => {
            for line in &args[1..] {
                writeln!(out, "{line}").unwrap();
            }
            out.flush().unwrap();
        }
        Some("exit") => process::exit(args[1].parse().expect("a code")),
        Some("hang") => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        Some("sum") => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            let sum = bytes.iter().fold(0u64, |sum, b| sum.wrapping_add(u64::from(*b)));
            writeln!(out, "sum {} {sum}", bytes.len()).unwrap();
            out.flush().unwrap();
        }
        Some("long") => {
            let n: usize = args[1].parse().expect("a length");
            writeln!(out, "{}", "x".repeat(n)).unwrap();
            writeln!(out, "after").unwrap();
            out.flush().unwrap();
        }
        _ => process::exit(99),
    }
}
