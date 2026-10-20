//! A stand-in for scrcpy, used by the integration tests (never shipped).
//!
//! It records the arguments it was started with (`scrcpy-args.txt`, one per
//! line) and the `ADB` environment variable (`scrcpy-adb.txt`), prints a line,
//! and then either ends after a moment or, when `scrcpy-mode.txt` says
//! `fail`, prints an error and exits with a failure code.
//!
//! Started with `--tunnel-port=<port>` it behaves like scrcpy with a forward
//! tunnel: it connects to that port, reads the first connection (the video
//! socket) to its end into `scrcpy-received.bin`, opens the audio socket (unless
//! `--no-audio`) and then the control socket, sends a control message (collapse
//! the panels) and keeps the device's answer in `scrcpy-control-reply.bin`, and
//! then stays until `scrcpy-release.txt` exists (at most 20 seconds), so a test
//! can look at the running session. With `scrcpy-split.bin` it sends that
//! message in two parts instead (see below); with `scrcpy-burst.txt` it opens
//! many connections at once.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fs, process, thread};

/// The port named by `--tunnel-port=<port>`, if the arguments have one.
fn tunnel_port(args: &[String]) -> Option<u16> {
    args.iter().find_map(|a| a.strip_prefix("--tunnel-port=")?.parse().ok())
}

fn connect(port: u16) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => return stream,
            Err(e) if Instant::now() > deadline => panic!("cannot connect to the tunnel: {e}"),
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn act_as_scrcpy_through_a_tunnel(dir: &Path, port: u16, args: &[String]) {
    let mut video = connect(port);
    let mut received = Vec::new();
    let _ = video.read_to_end(&mut received);
    fs::write(dir.join("scrcpy-received.bin"), received).expect("write the video bytes");

    if dir.join("scrcpy-burst.txt").exists() {
        // Many connections one right after the other, as scrcpy opens its audio
        // and control sockets: each says which one it is with its first byte.
        let _kept: Vec<TcpStream> = (0u8..20)
            .map(|i| {
                let mut stream = connect(port);
                stream.write_all(&[i]).expect("send the number of the connection");
                stream
            })
            .collect();
        wait_for_release(dir);
        return;
    }

    // The audio socket, unless it is off: the device starts it with a codec id.
    let _audio = if args.iter().any(|a| a == "--no-audio") {
        None
    } else {
        let mut audio = connect(port);
        let mut codec = [0u8; 4];
        let _ = audio.read_exact(&mut codec);
        Some(audio)
    };

    let mut control = connect(port);
    if let Ok(message) = fs::read(dir.join("scrcpy-split.bin")) {
        // One message in two parts with a pause between them, to be written
        // when `scrcpy-split-go.txt` exists: a message cut in the middle.
        control.write_all(&message[..10]).expect("send the first part");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.join("scrcpy-split-go.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        control.write_all(&message[10..]).expect("send the rest");
    } else {
        // One whole control message (collapse the panels), and the device's answer.
        control.write_all(&[7]).expect("send a control message");
        let mut reply = vec![0u8; 12];
        let _ = control.read_exact(&mut reply);
        fs::write(dir.join("scrcpy-control-reply.bin"), reply).expect("write the control reply");
    }
    wait_for_release(dir);
}

fn wait_for_release(dir: &Path) {

    let deadline = Instant::now() + Duration::from_secs(20);
    while !dir.join("scrcpy-release.txt").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

fn main() {
    let dir: PathBuf = env::current_exe()
        .expect("own path")
        .parent()
        .expect("parent dir")
        .to_path_buf();
    let args: Vec<String> = env::args().skip(1).collect();

    fs::write(dir.join("scrcpy-args.txt"), args.join("\n")).expect("write args");
    fs::write(dir.join("scrcpy-adb.txt"), env::var("ADB").unwrap_or_default()).expect("write adb");
    println!("INFO: fake scrcpy started");

    let mode = fs::read_to_string(dir.join("scrcpy-mode.txt")).unwrap_or_default();
    if mode.trim() == "fail" {
        eprintln!("ERROR: Could not connect to the device");
        process::exit(1);
    }
    if let Some(port) = tunnel_port(&args) {
        act_as_scrcpy_through_a_tunnel(&dir, port, &args);
        return;
    }
    thread::sleep(Duration::from_millis(400));
}
