//! The programs that add-ons start (`ProcessTable`), run for real against a stand-in program
//! (`examples/fake_filter.rs`). The stand-in does what its first argument says; nothing is taken from a
//! device. Every wait has a limit of time, so a table that misbehaves fails a test and does not hang it.

use scrcpy_agent_lib::core::addons::process::{
    MAX_LINE_BYTES, MAX_PROCESSES_PER_ADDON, ProcessEvent, ProcessId, ProcessListener, ProcessSpec, ProcessTable,
    SpawnError, WriteError,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where Cargo put the example programs next to this test executable.
fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("test executable path");
    let target_dir = exe.parent().and_then(Path::parent).expect("target directory");
    let path = target_dir.join("examples").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "{} has not been built. Run the full `cargo test`: a single test target does not build the examples.",
        path.display()
    );
    path
}

fn spec(args: &[&str]) -> ProcessSpec {
    ProcessSpec { command: example("fake_filter").to_string_lossy().into_owned(), args: args.iter().map(|a| a.to_string()).collect() }
}

/// What a program told, in the order it did.
#[derive(Clone, Default)]
struct Told(Arc<Mutex<Vec<ProcessEvent>>>);

impl Told {
    fn listener(&self) -> ProcessListener {
        let events = self.0.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    }
    fn all(&self) -> Vec<ProcessEvent> {
        self.0.lock().unwrap().clone()
    }
    /// Waits (at most ten seconds) until the program has ended, and returns everything it told.
    fn until_ended(&self) -> Vec<ProcessEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let all = self.all();
            if matches!(all.last(), Some(ProcessEvent::Exited(_))) {
                return all;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.all()
    }
}

fn start(table: &ProcessTable, addon: &str, args: &[&str]) -> (ProcessId, Told) {
    let told = Told::default();
    let id = table.spawn(addon, spec(args), told.listener());
    assert!(id.is_ok(), "starting {args:?}: {id:?}");
    (id.unwrap(), told)
}

fn line(text: &str) -> ProcessEvent {
    ProcessEvent::Line(text.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lines_a_program_writes_come_in_order_and_its_end_comes_last_with_its_code() {
    let table = ProcessTable::new();
    let (_, told) = start(&table, "vision", &["lines", "one", "two", "three"]);
    assert_eq!(told.until_ended(), vec![line("one"), line("two"), line("three"), ProcessEvent::Exited(Some(0))]);
    let (_, told) = start(&table, "vision", &["exit", "3"]);
    assert_eq!(told.until_ended(), vec![ProcessEvent::Exited(Some(3))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_byte_written_reaches_the_program_unchanged_and_closing_the_input_lets_it_finish() {
    let table = ProcessTable::new();
    let (id, told) = start(&table, "vision", &["sum"]);
    // All 256 values of a byte, in two writes: nothing is read as text.
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(table.write("vision", id, all[..100].to_vec()), Ok(()));
    assert_eq!(table.write("vision", id, all[100..].to_vec()), Ok(()));
    assert!(table.close_input("vision", id));
    let expected: u64 = (0..=255u64).sum();
    assert_eq!(told.until_ended(), vec![line(&format!("sum 256 {expected}")), ProcessEvent::Exited(Some(0))]);
    // Once it has ended there is nobody to write to.
    assert_eq!(table.write("vision", id, vec![1]), Err(WriteError::NotRunning));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_does_not_end_is_ended_by_kill_once_and_tells_that_it_ended_without_a_code() {
    let table = ProcessTable::new();
    let (id, told) = start(&table, "vision", &["hang"]);
    assert_eq!(table.running("vision"), 1);
    assert!(table.kill("vision", id));
    assert_eq!(told.until_ended(), vec![ProcessEvent::Exited(None)]);
    assert!(!table.kill("vision", id), "there was nothing left to end");
    assert_eq!(table.running("vision"), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_belongs_to_the_add_on_that_started_it() {
    let table = ProcessTable::new();
    let (id, told) = start(&table, "mine", &["hang"]);
    assert_eq!(table.write("theirs", id, vec![1]), Err(WriteError::NotRunning));
    assert!(!table.close_input("theirs", id));
    assert!(!table.kill("theirs", id));
    assert_eq!(table.running("mine"), 1);
    assert_eq!(told.all(), vec![], "nothing happened to it");
    assert!(table.kill("mine", id));
    told.until_ended();
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_an_add_on_ends_its_programs_and_only_its_programs() {
    let table = ProcessTable::new();
    let (_, first) = start(&table, "gone", &["hang"]);
    let (_, second) = start(&table, "gone", &["hang"]);
    let (kept, other) = start(&table, "stays", &["hang"]);
    table.remove_addon("gone");
    assert_eq!(first.until_ended(), vec![ProcessEvent::Exited(None)]);
    assert_eq!(second.until_ended(), vec![ProcessEvent::Exited(None)]);
    assert_eq!((table.running("gone"), table.running("stays")), (0, 1));
    assert_eq!(other.all(), vec![]);
    assert!(table.kill("stays", kept));
    other.until_ended();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_add_on_may_have_only_so_many_programs_running_and_a_slot_comes_back_when_one_ends() {
    let table = ProcessTable::new();
    let mut ids = Vec::new();
    for _ in 0..MAX_PROCESSES_PER_ADDON {
        ids.push(start(&table, "busy", &["hang"]));
    }
    let refused = table.spawn("busy", spec(&["hang"]), Told::default().listener());
    assert_eq!(refused, Err(SpawnError::TooMany));
    // Another add-on has its own count.
    let (other, other_told) = start(&table, "calm", &["hang"]);
    assert!(table.kill("calm", other));
    other_told.until_ended();
    // One ends, and its place is free again.
    let (id, told) = ids.remove(0);
    assert!(table.kill("busy", id));
    told.until_ended();
    let again = table.spawn("busy", spec(&["hang"]), Told::default().listener());
    assert!(again.is_ok(), "{again:?}");
    table.remove_addon("busy");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_that_cannot_be_started_is_refused_with_the_reason_and_takes_no_slot() {
    let table = ProcessTable::new();
    let told = Told::default();
    let missing = ProcessSpec { command: "no-such-program-for-the-test".into(), args: vec![] };
    assert_eq!(table.spawn("vision", missing, told.listener()), Err(SpawnError::NotFound));
    let empty = ProcessSpec { command: String::new(), args: vec![] };
    assert_eq!(table.spawn("vision", empty, told.listener()), Err(SpawnError::InvalidCommand));
    let nul = ProcessSpec { command: "a\0b".into(), args: vec![] };
    assert_eq!(table.spawn("vision", nul, told.listener()), Err(SpawnError::InvalidCommand));
    let nul_arg = ProcessSpec { command: example("fake_filter").to_string_lossy().into_owned(), args: vec!["x\0y".into()] };
    assert_eq!(table.spawn("vision", nul_arg, told.listener()), Err(SpawnError::InvalidCommand));
    assert_eq!(table.running("vision"), 0);
    assert_eq!(told.all(), vec![], "a program that did not start tells nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_line_longer_than_the_limit_is_cut_and_the_lines_after_it_still_come() {
    let table = ProcessTable::new();
    let (_, told) = start(&table, "vision", &["long", &(MAX_LINE_BYTES * 3).to_string()]);
    let all = told.until_ended();
    assert_eq!(all.len(), 3, "{} events", all.len());
    assert_eq!(all[0], line(&"x".repeat(MAX_LINE_BYTES)));
    assert_eq!(&all[1..], &[line("after"), ProcessEvent::Exited(Some(0))]);
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_an_add_on_through_the_core_ends_its_programs() {
    use scrcpy_agent_lib::core::Core;
    let dir = std::env::temp_dir().join(format!("scrcpy-agent-addon-process-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let core = Core::new(dir.clone(), Arc::new(|_event| {}));
    let table = core.addons().processes().clone();
    let (_, mine) = start(&table, "vision", &["hang"]);
    let (other_id, other) = start(&table, "other", &["hang"]);
    core.remove_addon("vision");
    assert_eq!(mine.until_ended(), vec![ProcessEvent::Exited(None)]);
    assert_eq!(table.running("other"), 1, "the programs of another add-on go on");
    assert!(table.kill("other", other_id));
    other.until_ended();
    let _ = std::fs::remove_dir_all(&dir);
}
