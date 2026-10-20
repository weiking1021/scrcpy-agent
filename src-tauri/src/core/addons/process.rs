//! The programs that add-ons start on this computer (the permission `run_process`).
//!
//! A program is started once and kept: the add-on writes to its standard input as long as it likes and is
//! told of each line it writes to its standard output and of its end. Nothing goes through a shell: the
//! command and each argument are separate, so no text is ever read as more than what it is. Every program
//! belongs to the add-on that started it; no other add-on can write to it or end it. When an add-on is
//! stopped, removed or switched off, its programs are ended with it.
//!
//! Input to a program waits in a queue of limited size (`INPUT_QUEUE`): a program that reads slower than it is
//! fed (a video, for one) makes a write fail with `WriteError::Full` and the caller drops that piece, so memory
//! does not grow without end.

use std::collections::HashMap;
use std::io;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Notify, mpsc};

/// The most programs one add-on may have running at one time.
pub const MAX_PROCESSES_PER_ADDON: usize = 4;
/// The longest line of a program that is told to the add-on; the rest of a longer line is dropped.
pub const MAX_LINE_BYTES: usize = 64 * 1024;
/// How many writes may wait for a program that is slow to read.
pub const INPUT_QUEUE: usize = 256;

/// Windows: do not open a console window for the program.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A program an add-on started, by the number the table gave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProcessId(pub u32);

/// What to start: the program and its arguments, each as it is (no shell reads them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSpec {
    pub command: String,
    pub args: Vec<String>,
}

/// What a running program tells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessEvent {
    /// A line of its standard output, without the line end. Bytes that are not text are replaced.
    Line(String),
    /// It ended, and this is always the last event: with its exit code, or `None` when it ended without one
    /// (it was ended by this program, or by a signal).
    Exited(Option<i32>),
}

/// Who is told of what a program does. It is called from the program's own task, in order.
pub type ProcessListener = Arc<dyn Fn(ProcessEvent) + Send + Sync>;

/// Why a program was not started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// The command is empty, or it or an argument holds a NUL character.
    InvalidCommand,
    /// There is no such program.
    NotFound,
    /// The system does not let this program be run.
    NotAllowed,
    /// The add-on has `MAX_PROCESSES_PER_ADDON` running already.
    TooMany,
    /// The system could not start it for another reason.
    Failed,
}

/// Why nothing was written to a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// It is not running (it ended, its input was closed, or it is not this add-on's).
    NotRunning,
    /// The program has not read what was written before: this piece was not kept.
    Full,
}

struct Entry {
    id: ProcessId,
    /// Where the input goes; `None` once it was closed.
    input: Option<mpsc::Sender<Vec<u8>>>,
    kill: Arc<Notify>,
}

#[derive(Default)]
struct Inner {
    next: AtomicU32,
    /// The running programs of each add-on.
    running: Mutex<HashMap<String, Vec<Entry>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Inner {
    /// Forgets a program that ended (if it is still known).
    fn forget(&self, addon: &str, id: ProcessId) {
        let mut running = locked(&self.running);
        if let Some(list) = running.get_mut(addon) {
            list.retain(|e| e.id != id);
            if list.is_empty() {
                running.remove(addon);
            }
        }
    }
}

/// The programs that are running, for all add-ons.
pub struct ProcessTable {
    inner: Arc<Inner>,
}

impl ProcessTable {
    pub fn new() -> Arc<Self> {
        Arc::new(ProcessTable { inner: Arc::new(Inner::default()) })
    }

    /// Starts a program for an add-on. Must be called inside a Tokio runtime, which also runs the tasks that
    /// read the program's output.
    pub fn spawn(&self, addon: &str, spec: ProcessSpec, listener: ProcessListener) -> Result<ProcessId, SpawnError> {
        if spec.command.is_empty() || spec.command.contains('\0') || spec.args.iter().any(|a| a.contains('\0')) {
            return Err(SpawnError::InvalidCommand);
        }
        // The place is taken before the program starts, so two starts at once cannot go over the limit.
        let id = ProcessId(self.inner.next.fetch_add(1, Ordering::Relaxed) + 1);
        let kill = Arc::new(Notify::new());
        let (input, queue) = mpsc::channel::<Vec<u8>>(INPUT_QUEUE);
        {
            let mut running = locked(&self.inner.running);
            let list = running.entry(addon.to_string()).or_default();
            if list.len() >= MAX_PROCESSES_PER_ADDON {
                return Err(SpawnError::TooMany);
            }
            list.push(Entry { id, input: Some(input), kill: kill.clone() });
        }

        let mut command = Command::new(&spec.command);
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(CREATE_NO_WINDOW);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.inner.forget(addon, id);
                return Err(match error.kind() {
                    io::ErrorKind::NotFound => SpawnError::NotFound,
                    io::ErrorKind::PermissionDenied => SpawnError::NotAllowed,
                    _ => SpawnError::Failed,
                });
            }
        };
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            self.inner.forget(addon, id);
            return Err(SpawnError::Failed);
        };

        tokio::spawn(write_input(stdin, queue));
        let inner = self.inner.clone();
        let addon = addon.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            let mut killed = false;
            let mut buffer = Vec::new();
            loop {
                tokio::select! {
                    read = read_line(&mut lines, &mut buffer) => match read {
                        Ok(true) => listener(ProcessEvent::Line(String::from_utf8_lossy(&buffer).into_owned())),
                        Ok(false) | Err(_) => break,
                    },
                    () = kill.notified() => {
                        killed = true;
                        let _ = child.start_kill();
                        break;
                    }
                }
            }
            let status = child.wait().await.ok();
            // It is no longer running before anyone is told so.
            inner.forget(&addon, id);
            let code = if killed { None } else { status.and_then(|s| s.code()) };
            listener(ProcessEvent::Exited(code));
        });
        Ok(id)
    }

    /// Writes bytes to the standard input of a program of this add-on.
    pub fn write(&self, addon: &str, id: ProcessId, bytes: Vec<u8>) -> Result<(), WriteError> {
        let running = locked(&self.inner.running);
        let input = running
            .get(addon)
            .and_then(|list| list.iter().find(|e| e.id == id))
            .and_then(|e| e.input.as_ref())
            .ok_or(WriteError::NotRunning)?;
        input.try_send(bytes).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => WriteError::Full,
            mpsc::error::TrySendError::Closed(_) => WriteError::NotRunning,
        })
    }

    /// Closes the standard input of a program of this add-on (a program that reads to its end then ends).
    pub fn close_input(&self, addon: &str, id: ProcessId) -> bool {
        let mut running = locked(&self.inner.running);
        running
            .get_mut(addon)
            .and_then(|list| list.iter_mut().find(|e| e.id == id))
            .and_then(|e| e.input.take())
            .is_some()
    }

    /// Ends a program of this add-on; whether there was one running.
    pub fn kill(&self, addon: &str, id: ProcessId) -> bool {
        let running = locked(&self.inner.running);
        match running.get(addon).and_then(|list| list.iter().find(|e| e.id == id)) {
            Some(entry) => {
                entry.kill.notify_one();
                true
            }
            None => false,
        }
    }

    /// Whether the add-on has this program running.
    pub fn owns(&self, addon: &str, id: ProcessId) -> bool {
        locked(&self.inner.running).get(addon).is_some_and(|list| list.iter().any(|e| e.id == id))
    }

    /// How many programs of this add-on are running.
    pub fn running(&self, addon: &str) -> usize {
        locked(&self.inner.running).get(addon).map_or(0, Vec::len)
    }

    /// Ends every program of the add-on.
    pub fn remove_addon(&self, addon: &str) {
        let entries = locked(&self.inner.running).remove(addon).unwrap_or_default();
        for entry in entries {
            entry.kill.notify_one();
        }
    }
}

/// Writes what is queued to the program until the queue is closed (or the program no longer reads), then closes
/// its input.
async fn write_input(mut stdin: tokio::process::ChildStdin, mut queue: mpsc::Receiver<Vec<u8>>) {
    while let Some(bytes) = queue.recv().await {
        if stdin.write_all(&bytes).await.is_err() {
            break;
        }
    }
    let _ = stdin.shutdown().await;
}

/// Reads one line into `line` (without its end): at most `MAX_LINE_BYTES` of it are kept, the rest is read and
/// dropped. `false` at the end of the output (a last line without an end counts as a line).
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    let mut any = false;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(any);
        }
        any = true;
        let (chunk, found) = match available.iter().position(|b| *b == b'\n') {
            Some(at) => (&available[..at], true),
            None => (available, false),
        };
        let room = MAX_LINE_BYTES.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let used = chunk.len() + usize::from(found);
        reader.consume(used);
        if found {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(true);
        }
    }
}
