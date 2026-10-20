//! One add-on running: its `main.js` in a script runtime of its own, in a thread of its own.
//!
//! What a script can do is the object `host` and a small `console`; each call that needs a permission
//! is checked when it is made. A callback (an entry of the menu that was chosen) runs on the thread of
//! its add-on, one at a time in the order they came, with a limit of time; one that fails is logged and
//! does not stop the add-on. No error of a script is a panic of the program.

use super::host::{AddonHost, AddonLogLevel};
use super::limits::Limits;
use super::timers::Timers;
use crate::core::addons::events::{AddonEvent, EVENT_NAMES};
use crate::core::addons::api::{MenuRegisterCode, ProcessErrorCode, ScreenErrorCode, code_text};
use crate::core::addons::buttons::ButtonHandler;
use crate::core::addons::menu::{AppMenuHandler, MenuHandler, MenuRegisterError};
use crate::core::addons::permission::{AddonPermission, Granted};
use crate::core::addons::process::{ProcessEvent, ProcessId, ProcessListener, ProcessSpec, SpawnError};
use crate::core::addons::screen_pipe::PipeError;
use rquickjs::convert::Coerced;
use rquickjs::function::Async;
use rquickjs::{AsyncContext, AsyncRuntime, CatchResultExt, Ctx, FromJs, Function, Object, Persistent, Promise, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, mpsc as tokio_mpsc};

/// The code that makes `host`, `console` and the timers out of what the program gives the script.
const PRELUDE: &str = include_str!("prelude.js");

/// What the prelude is told a callback is of (the same words as in `prelude.js`).
const MENU: &str = "menu";
const APP_MENU: &str = "appMenu";
const TOOLBAR: &str = "toolbar";
const PROCESS_LINE: &str = "processLine";
const PROCESS_EXIT: &str = "processExit";
const TIMER: &str = "timer";
const EVENT: &str = "event";

/// The levels of the log as the prelude numbers them.
const LEVEL_WARN: u8 = 1;
const LEVEL_ERROR: u8 = 2;

/// How long past its limit a callback is waited for, so that the interruption is what ends it.
const GRACE: Duration = Duration::from_millis(250);

/// The longest delay of a timer (a day): a longer one is made this long.
const MAX_DELAY_MS: f64 = 86_400_000.0;

/// Why an add-on did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// `main.js` did not parse or threw (the text is the script's own, for showing only).
    Script(String),
    /// `main.js` ran longer than `Limits::init_time`.
    Timeout,
    /// The thread or the runtime could not be made (the text is the system's, for showing only).
    Thread(String),
}

/// What the thread of an add-on is asked to do.
enum Task {
    /// An entry of the menu was chosen for a device.
    Menu { key: u32, device: String },
    /// An entry of the menu of an app was chosen, or a button of the toolbar pressed. The callback is given the
    /// arguments (a JSON array) one by one.
    Place { kind: &'static str, key: u32, arguments: String },
}

/// The arguments of a callback of the menu of an app or the toolbar, as the prelude takes them.
fn arguments(device: &str, app: Option<&str>) -> String {
    serde_json::json!([device, app]).to_string()
}

/// What the thread of an add-on and the code that calls into it share.
struct State {
    addon: String,
    host: Arc<dyn AddonHost>,
    granted: Granted,
    limits: Limits,
    /// When what runs now has to be over; `None` while nothing runs.
    deadline: Mutex<Option<Instant>>,
    stop: AtomicBool,
    stopped: Notify,
    /// Set by the interruption when it was the time that ran out.
    timed_out: AtomicBool,
    timers: Mutex<Timers>,
    /// The id of each entry of the menu, for the log.
    menu_ids: Mutex<HashMap<u32, String>>,
    tasks: tokio_mpsc::UnboundedSender<Task>,
    /// The events that wait for the add-on (the oldest first); more than `Limits::max_pending_events` are not kept.
    events: Mutex<VecDeque<AddonEvent>>,
    event_ready: Notify,
    /// How many events were dropped since the log last said so.
    dropped_events: AtomicUsize,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A text of at most `max` characters; a longer one is cut and ends with `…`.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max).chain(['…']).collect() }
}

impl State {
    fn log(&self, level: AddonLogLevel, text: &str) {
        self.host.log(&self.addon, level, &cut(text, self.limits.max_log_chars));
    }

    fn set_deadline(&self, time: Duration) {
        *locked(&self.deadline) = Some(Instant::now() + time);
    }

    fn clear_deadline(&self) {
        *locked(&self.deadline) = None;
    }

    /// Asked by the engine from time to time while a script runs: whether it has to end now.
    fn interrupted(&self) -> bool {
        if self.stop.load(Ordering::SeqCst) {
            return true;
        }
        let late = locked(&self.deadline).is_some_and(|deadline| Instant::now() > deadline);
        if late {
            self.timed_out.store(true, Ordering::SeqCst);
        }
        late
    }

    /// Puts an event in the queue; when there are more than the limit, the oldest are dropped (and counted).
    fn push_event(&self, event: AddonEvent) {
        {
            let mut events = locked(&self.events);
            events.push_back(event);
            while events.len() > self.limits.max_pending_events {
                events.pop_front();
                self.dropped_events.fetch_add(1, Ordering::SeqCst);
            }
        }
        self.event_ready.notify_one();
    }

    fn pop_event(&self) -> Option<AddonEvent> {
        locked(&self.events).pop_front()
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stopped.notify_one();
    }

    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// A running add-on. Dropping it stops it.
pub struct AddonRuntime {
    state: Arc<State>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for AddonRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddonRuntime").field("addon", &self.state.addon).finish_non_exhaustive()
    }
}

impl AddonRuntime {
    /// Runs `script` as the `main.js` of `addon`, and returns when it has run to its end (what it
    /// started with promises goes on). If it fails, what it registered is taken away again.
    pub fn start(
        addon: &str,
        script: &str,
        granted: Granted,
        host: Arc<dyn AddonHost>,
        limits: Limits,
    ) -> Result<AddonRuntime, StartError> {
        let (tasks, receiver) = tokio_mpsc::unbounded_channel();
        let state = Arc::new(State {
            addon: addon.to_string(),
            host,
            granted,
            limits,
            deadline: Mutex::new(None),
            stop: AtomicBool::new(false),
            stopped: Notify::new(),
            timed_out: AtomicBool::new(false),
            timers: Mutex::new(Timers::new(limits.max_timers, limits.min_interval)),
            menu_ids: Mutex::new(HashMap::new()),
            tasks,
            events: Mutex::new(VecDeque::new()),
            event_ready: Notify::new(),
            dropped_events: AtomicUsize::new(0),
        });
        let (ready, started) = mpsc::channel();
        let (thread_state, script) = (state.clone(), script.to_string());
        let thread = std::thread::Builder::new()
            .name(format!("addon-{addon}"))
            .spawn(move || thread_main(thread_state, script, receiver, ready))
            .map_err(|e| StartError::Thread(e.to_string()))?;
        match started.recv() {
            Ok(Ok(())) => Ok(AddonRuntime { state, thread: Some(thread) }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                Err(StartError::Thread("the thread of the add-on ended before it started".into()))
            }
        }
    }

    /// Tells it of an event: the callbacks it subscribed to this event by (`host.on`) run, one at a time with the
    /// others, in the order of the subscriptions. When it is busy and too many events wait, the oldest are
    /// dropped and the log says how many.
    pub fn emit(&self, event: AddonEvent) {
        self.state.push_event(event);
    }

    /// Stops it: what it registered is taken away, a callback that is running is interrupted, and the
    /// thread has ended when this returns.
    pub fn stop(self) {}
}

impl Drop for AddonRuntime {
    fn drop(&mut self) {
        self.state.request_stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The thread of an add-on: a runtime for the waiting, and the script runtime in it.
fn thread_main(
    state: Arc<State>,
    script: String,
    tasks: tokio_mpsc::UnboundedReceiver<Task>,
    ready: mpsc::Sender<Result<(), StartError>>,
) {
    let waiting = tokio::runtime::Builder::new_current_thread().enable_all().build();
    match waiting {
        Ok(waiting) => waiting.block_on(run(state, script, tasks, ready)),
        Err(e) => {
            let _ = ready.send(Err(StartError::Thread(e.to_string())));
        }
    }
}

/// The words the script switches on, from the types of `core::addons::api` (which are also the TypeScript types the
/// authors read).
fn menu_error_code(error: MenuRegisterError) -> String {
    code_text(&MenuRegisterCode::from(error))
}

fn spawn_error_code(error: SpawnError) -> String {
    code_text(&ProcessErrorCode::from(error))
}

fn pipe_error_code(error: PipeError) -> String {
    code_text(&ScreenErrorCode::from(error))
}

/// What the script runtime gives the prelude: the functions that reach the program.
fn install<'js>(ctx: &Ctx<'js>, state: &Arc<State>) -> rquickjs::Result<Object<'js>> {
    let native = Object::new(ctx.clone())?;

    let s = state.clone();
    native.set(
        "log",
        Function::new(ctx.clone(), move |level: u8, text: String| {
            let level = match level {
                LEVEL_WARN => AddonLogLevel::Warn,
                LEVEL_ERROR => AddonLogLevel::Error,
                _ => AddonLogLevel::Info,
            };
            s.log(level, &text);
        })?,
    )?;

    let s = state.clone();
    native.set(
        "registerMenu",
        Function::new(ctx.clone(), move |id: String, label: String, key: u32| -> Option<String> {
            let tasks = s.tasks.clone();
            let handler: MenuHandler = Arc::new(move |device: &str| {
                let _ = tasks.send(Task::Menu { key, device: device.to_string() });
            });
            match s.host.register_device_menu_item(&s.addon, &id, &label, handler) {
                Ok(()) => {
                    locked(&s.menu_ids).insert(key, id);
                    None
                }
                Err(error) => Some(menu_error_code(error).to_string()),
            }
        })?,
    )?;

    let s = state.clone();
    native.set(
        "registerAppMenu",
        Function::new(ctx.clone(), move |id: String, label: String, key: u32| -> Option<String> {
            let tasks = s.tasks.clone();
            let handler: AppMenuHandler = Arc::new(move |device: &str, app: &str| {
                let arguments = arguments(device, Some(app));
                let _ = tasks.send(Task::Place { kind: APP_MENU, key, arguments });
            });
            match s.host.register_app_menu_item(&s.addon, &id, &label, handler) {
                Ok(()) => {
                    locked(&s.menu_ids).insert(key, id);
                    None
                }
                Err(error) => Some(menu_error_code(error).to_string()),
            }
        })?,
    )?;

    let s = state.clone();
    native.set(
        "registerToolbar",
        Function::new(ctx.clone(), move |id: String, label: String, key: u32| -> Option<String> {
            let tasks = s.tasks.clone();
            let handler: ButtonHandler = Arc::new(move |device: &str, app: Option<&str>| {
                let arguments = arguments(device, app);
                let _ = tasks.send(Task::Place { kind: TOOLBAR, key, arguments });
            });
            match s.host.register_toolbar_button(&s.addon, &id, &label, handler) {
                Ok(()) => {
                    locked(&s.menu_ids).insert(key, id);
                    None
                }
                Err(error) => Some(menu_error_code(error).to_string()),
            }
        })?,
    )?;

    let s = state.clone();
    native.set(
        "spawnProcess",
        Function::new(ctx.clone(), move |command: String, args: String, key: u32| -> String {
            if let Err(denied) = s.granted.check(AddonPermission::RunProcess) {
                s.host.permission_denied(&s.addon, denied.0);
                return serde_json::json!({ "denied": denied.0 }).to_string();
            }
            // The arguments are made by the prelude (a list of text); anything else is a mistake of this program.
            let Ok(args) = serde_json::from_str::<Vec<String>>(&args) else {
                return serde_json::json!({ "ok": false, "code": spawn_error_code(SpawnError::InvalidCommand) }).to_string();
            };
            let tasks = s.tasks.clone();
            let listener: ProcessListener = Arc::new(move |event| {
                let (kind, arguments) = match event {
                    ProcessEvent::Line(line) => (PROCESS_LINE, serde_json::json!([line])),
                    ProcessEvent::Exited(code) => (PROCESS_EXIT, serde_json::json!([code])),
                };
                let _ = tasks.send(Task::Place { kind, key, arguments: arguments.to_string() });
            });
            match s.host.spawn_process(&s.addon, ProcessSpec { command, args }, listener) {
                Ok(id) => {
                    locked(&s.menu_ids).insert(key, format!("program {}", id.0));
                    serde_json::json!({ "ok": true, "id": id.0 }).to_string()
                }
                Err(error) => serde_json::json!({ "ok": false, "code": spawn_error_code(error) }).to_string(),
            }
        })?,
    )?;

    let s = state.clone();
    native.set(
        "killProcess",
        Function::new(ctx.clone(), move |id: u32| s.host.kill_process(&s.addon, ProcessId(id)))?,
    )?;

    let s = state.clone();
    native.set(
        "closeProcessInput",
        Function::new(ctx.clone(), move |id: u32| s.host.close_process_input(&s.addon, ProcessId(id)))?,
    )?;

    let s = state.clone();
    native.set(
        "pipeScreen",
        Function::new(ctx.clone(), move |device: String, id: u32, app: Option<String>| -> String {
            if let Err(denied) = s.granted.check(AddonPermission::ScreenCapture) {
                s.host.permission_denied(&s.addon, denied.0);
                return serde_json::json!({ "denied": denied.0 }).to_string();
            }
            match s.host.pipe_screen(&s.addon, &device, app.as_deref(), ProcessId(id)) {
                Ok(()) => serde_json::json!({ "ok": true }).to_string(),
                Err(error) => serde_json::json!({ "ok": false, "code": pipe_error_code(error) }).to_string(),
            }
        })?,
    )?;

    let s = state.clone();
    native.set(
        "getSetting",
        // The value as JSON text, or nothing for a setting the add-on did not declare.
        Function::new(ctx.clone(), move |key: String| -> Option<String> {
            s.host.setting(&s.addon, &key).and_then(|value| serde_json::to_string(&value).ok())
        })?,
    )?;

    let s = state.clone();
    native.set(
        "getFolder",
        // The path as text; nothing when the add-on is not installed or the path is not valid text.
        Function::new(ctx.clone(), move || -> Option<String> {
            s.host.folder(&s.addon).and_then(|folder| folder.to_str().map(String::from))
        })?,
    )?;

    native.set(
        "isEvent",
        Function::new(ctx.clone(), |name: String| EVENT_NAMES.contains(&name.as_str()))?,
    )?;

    let s = state.clone();
    native.set(
        "startTimer",
        Function::new(ctx.clone(), move |key: u32, delay_ms: f64, repeat: bool| -> Option<String> {
            let delay = Duration::from_secs_f64(delay_ms.clamp(0.0, MAX_DELAY_MS) / 1000.0);
            let started = locked(&s.timers).start(key, delay, repeat, Instant::now());
            started.err().map(|_| "limit".to_string())
        })?,
    )?;

    let s = state.clone();
    native.set(
        "clearTimer",
        Function::new(ctx.clone(), move |key: u32| {
            locked(&s.timers).clear(key);
        })?,
    )?;

    let s = state.clone();
    native.set(
        "shell",
        Function::new(
            ctx.clone(),
            Async(move |device: String, command: String| {
                let s = s.clone();
                async move {
                    if let Err(denied) = s.granted.check(AddonPermission::DeviceShell) {
                        s.host.permission_denied(&s.addon, denied.0);
                        return serde_json::json!({ "ok": false, "denied": denied.0 }).to_string();
                    }
                    match s.host.device_shell(&s.addon, &device, &command).await {
                        Ok(output) => serde_json::json!({ "ok": true, "output": output }).to_string(),
                        Err(error) => serde_json::json!({ "ok": false, "code": error.code() }).to_string(),
                    }
                }
            }),
        )?,
    )?;

    Ok(native)
}

/// How a callback ended.
enum Outcome {
    Done,
    Failed(String),
    TimedOut,
    Stopped,
}

/// What came of waiting for the promise of a callback.
enum Waited {
    Ended(Option<String>),
    Elapsed,
}

/// Runs one callback of the script (by the function the prelude gave) and waits for it, within the limit of time.
async fn invoke(
    context: &AsyncContext,
    dispatcher: &Persistent<Function<'static>>,
    state: &Arc<State>,
    kind: &'static str,
    key: u32,
    argument: String,
) -> Outcome {
    state.timed_out.store(false, Ordering::SeqCst);
    state.set_deadline(state.limits.call_time);
    let wait = state.limits.call_time + GRACE;
    let dispatcher = dispatcher.clone();
    let result = context
        .async_with(async move |ctx| -> Result<Waited, String> {
            let dispatch = dispatcher.restore(&ctx).catch(&ctx).map_err(|e| e.to_string())?;
            let promise: Promise = dispatch.call((kind, key, argument)).catch(&ctx).map_err(|e| e.to_string())?;
            match tokio::time::timeout(wait, promise.into_future::<Option<String>>()).await {
                Ok(ended) => Ok(Waited::Ended(ended.catch(&ctx).map_err(|e| e.to_string())?)),
                Err(_) => Ok(Waited::Elapsed),
            }
        })
        .await;
    state.clear_deadline();
    if state.stop_requested() {
        return Outcome::Stopped;
    }
    if state.timed_out.load(Ordering::SeqCst) {
        return Outcome::TimedOut;
    }
    match result {
        Ok(Waited::Ended(None)) => Outcome::Done,
        Ok(Waited::Ended(Some(text))) | Err(text) => Outcome::Failed(text),
        Ok(Waited::Elapsed) => Outcome::TimedOut,
    }
}

/// Tells the log how a callback ended, when it did not end well.
fn report(state: &State, what: &str, outcome: Outcome) {
    match outcome {
        Outcome::Done | Outcome::Stopped => {}
        Outcome::Failed(text) => state.log(AddonLogLevel::Error, &format!("{what}: {text}")),
        Outcome::TimedOut => state.log(
            AddonLogLevel::Error,
            &format!("{what}: it ran longer than {:?} and was stopped", state.limits.call_time),
        ),
    }
}

/// The promises that were rejected with nothing to catch them, and what they were rejected with.
type Rejections = Rc<RefCell<Vec<(Persistent<Value<'static>>, String)>>>;

/// Tells the log of the rejections that nothing caught, and forgets them.
fn report_rejections(state: &State, rejected: &Rejections) {
    let uncaught: Vec<(Persistent<Value<'static>>, String)> = rejected.borrow_mut().drain(..).collect();
    for (_, text) in uncaught {
        state.log(AddonLogLevel::Error, &format!("unhandled rejection: {text}"));
    }
}

async fn run(
    state: Arc<State>,
    script: String,
    mut tasks: tokio_mpsc::UnboundedReceiver<Task>,
    ready: mpsc::Sender<Result<(), StartError>>,
) {
    let fail = |error: StartError| {
        state.host.remove_registrations(&state.addon);
        let _ = ready.send(Err(error));
    };
    let runtime = match AsyncRuntime::new() {
        Ok(runtime) => runtime,
        Err(e) => return fail(StartError::Thread(e.to_string())),
    };
    runtime.set_memory_limit(state.limits.memory_bytes).await;
    let interruptible = state.clone();
    runtime.set_interrupt_handler(Some(Box::new(move || interruptible.interrupted()))).await;
    // A promise that is rejected with nothing to catch it is told of at once, and again if something catches it
    // a moment later (an async function that threw before its caller could wait for it). So it is kept until the
    // work that rejected it is over (`report_rejections`), and forgotten if it was caught by then.
    let rejected: Rejections = Rc::new(RefCell::new(Vec::new()));
    let tracked = rejected.clone();
    runtime
        .set_host_promise_rejection_tracker(Some(Box::new(move |ctx, promise, reason, handled| {
            let mut kept = tracked.borrow_mut();
            if handled {
                kept.retain(|(saved, _)| saved.clone().restore(&ctx).map_or(true, |saved| saved != promise));
            } else {
                let text = Coerced::<String>::from_js(&ctx, reason)
                    .map(|text| text.0)
                    .unwrap_or_else(|_| "a promise was rejected".to_string());
                kept.push((Persistent::save(&ctx, promise), text));
            }
        })))
        .await;
    let context = match AsyncContext::full(&runtime).await {
        Ok(context) => context,
        Err(e) => return fail(StartError::Thread(e.to_string())),
    };

    // The prelude, then main.js, within the time that main.js has.
    state.set_deadline(state.limits.init_time);
    let started = context
        .async_with({
            let state = state.clone();
            async move |ctx| -> Result<Persistent<Function<'static>>, String> {
                let native = install(&ctx, &state).catch(&ctx).map_err(|e| e.to_string())?;
                let make: Function = ctx.eval(PRELUDE).catch(&ctx).map_err(|e| e.to_string())?;
                let dispatcher: Function = make.call((native,)).catch(&ctx).map_err(|e| e.to_string())?;
                let dispatcher = Persistent::save(&ctx, dispatcher);
                ctx.eval::<(), _>(script).catch(&ctx).map_err(|e| e.to_string())?;
                Ok(dispatcher)
            }
        })
        .await;
    state.clear_deadline();
    report_rejections(&state, &rejected);
    let dispatcher = match started {
        Ok(dispatcher) => dispatcher,
        Err(_) if state.timed_out.load(Ordering::SeqCst) => return fail(StartError::Timeout),
        Err(text) => return fail(StartError::Script(text)),
    };
    let _ = ready.send(Ok(()));

    // Then it waits: for a menu entry to be chosen, for a timer, for a stop, and (all the time) lets
    // what the script started with promises go on.
    let mut driving = Box::pin(runtime.drive());
    while !state.stop_requested() {
        let next_timer = locked(&state.timers).next_due().map(tokio::time::Instant::from_std);
        let due = async {
            match next_timer {
                Some(time) => tokio::time::sleep_until(time).await,
                None => std::future::pending().await,
            }
        };
        let driven = std::future::poll_fn(|cx| {
            state.set_deadline(state.limits.call_time);
            let polled = driving.as_mut().poll(cx);
            state.clear_deadline();
            report_rejections(&state, &rejected);
            polled
        });
        tokio::select! {
            biased;
            () = state.stopped.notified() => break,
            task = tasks.recv() => match task {
                Some(Task::Menu { key, device }) => {
                    let what = match locked(&state.menu_ids).get(&key) {
                        Some(id) => format!("the entry \"{id}\" of the menu"),
                        None => "an entry of the menu".to_string(),
                    };
                    let outcome = invoke(&context, &dispatcher, &state, MENU, key, device).await;
                    report(&state, &what, outcome);
                    report_rejections(&state, &rejected);
                }
                Some(Task::Place { kind, key, arguments }) => {
                    let named = locked(&state.menu_ids).get(&key).cloned();
                    let what = match (kind, named) {
                        (PROCESS_LINE | PROCESS_EXIT, Some(name)) => format!("the callback of {name}"),
                        (PROCESS_LINE | PROCESS_EXIT, None) => "the callback of a program".to_string(),
                        (TOOLBAR, Some(id)) => format!("the entry \"{id}\" of the toolbar"),
                        (TOOLBAR, None) => "an entry of the toolbar".to_string(),
                        (_, Some(id)) => format!("the entry \"{id}\" of the menu of the apps"),
                        (_, None) => "an entry of the menu of the apps".to_string(),
                    };
                    let outcome = invoke(&context, &dispatcher, &state, kind, key, arguments).await;
                    report(&state, &what, outcome);
                    report_rejections(&state, &rejected);
                }
                None => break,
            },
            () = due => {
                let timers = locked(&state.timers).take_due(Instant::now());
                for key in timers {
                    if state.stop_requested() {
                        break;
                    }
                    let outcome = invoke(&context, &dispatcher, &state, TIMER, key, String::new()).await;
                    report(&state, "a timer", outcome);
                    report_rejections(&state, &rejected);
                }
            }
            () = state.event_ready.notified() => {
                while let Some(event) = state.pop_event() {
                    if state.stop_requested() {
                        break;
                    }
                    let dropped = state.dropped_events.swap(0, Ordering::SeqCst);
                    if dropped > 0 {
                        state.log(
                            AddonLogLevel::Warn,
                            &format!("{dropped} events were dropped: the add-on was too slow to be told of them"),
                        );
                    }
                    let argument = serde_json::json!({ "name": event.name(), "data": event.data() }).to_string();
                    let outcome = invoke(&context, &dispatcher, &state, EVENT, 0, argument).await;
                    report(&state, &format!("the event \"{}\"", event.name()), outcome);
                    report_rejections(&state, &rejected);
                }
            }
            () = driven => break,
        }
    }
    state.host.remove_registrations(&state.addon);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::addons::engine::host::{AddonLogLevel, HostFuture};
    use crate::core::addons::buttons::{ButtonHandler, ButtonId, ButtonRegistry};
    use crate::core::addons::menu::{
        AppMenu, AppMenuHandler, AppMenuItemId, DeviceMenu, MenuHandler, MenuItemId, MenuRegisterError,
    };
    use crate::core::addons::permission::AddonPermission;
    use crate::core::addons::process::{ProcessEvent, ProcessId, ProcessListener, ProcessSpec, SpawnError};
    use crate::core::addons::screen_pipe::PipeError;
    use crate::core::addons::settings::SettingValue;
    use crate::core::error::CoreError;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    // The scripts are made up for the tests. Everything the scripts ask of the program goes to a fake host
    // that writes down what it was asked; nothing here touches a device, a file or the network.

    const SHELL: AddonPermission = AddonPermission::DeviceShell;

    #[derive(Debug, Clone, PartialEq)]
    enum Event {
        Log(String, AddonLogLevel, String),
        Registered(String, String, String),
        AppRegistered(String, String, String),
        ToolbarRegistered(String, String, String),
        Removed(String),
        Shell(String, String, String),
        Denied(String, AddonPermission),
        Spawned(String, String, Vec<String>),
        Killed(String, ProcessId),
        InputClosed(String, ProcessId),
        Piped(String, String, Option<String>, ProcessId),
    }

    /// A host that keeps a real menu (so what the menu refuses is refused here too) and answers shell
    /// commands as it is told.
    struct FakeHost {
        events: Mutex<Vec<Event>>,
        menu: Mutex<DeviceMenu>,
        app_menu: Mutex<AppMenu>,
        toolbar: Mutex<ButtonRegistry>,
        shell_result: Mutex<Result<String, CoreError>>,
        shell_delay: Mutex<Duration>,
        /// How starting a program and feeding a screen are answered.
        spawn_result: Mutex<Result<(), SpawnError>>,
        pipe_result: Mutex<Result<(), PipeError>>,
        /// What each program started can be told by (the first started is number 0).
        listeners: Mutex<Vec<ProcessListener>>,
        /// The settings the user set for the add-on.
        settings: Mutex<std::collections::HashMap<String, SettingValue>>,
        /// Where the add-on is installed from (none: it is not installed).
        folder: Mutex<Option<std::path::PathBuf>>,
    }

    impl FakeHost {
        /// Tells the add-on, as the program number `index` that it started, of something it did.
        fn tell(&self, index: usize, event: ProcessEvent) {
            let listener = self.listeners.lock().unwrap()[index].clone();
            listener(event);
        }

        fn new() -> Arc<FakeHost> {
            Arc::new(FakeHost {
                events: Mutex::new(Vec::new()),
                menu: Mutex::new(DeviceMenu::new()),
                app_menu: Mutex::new(AppMenu::new()),
                toolbar: Mutex::new(ButtonRegistry::new()),
                shell_result: Mutex::new(Ok("shell output".into())),
                shell_delay: Mutex::new(Duration::ZERO),
                spawn_result: Mutex::new(Ok(())),
                pipe_result: Mutex::new(Ok(())),
                listeners: Mutex::new(Vec::new()),
                settings: Mutex::new(std::collections::HashMap::new()),
                folder: Mutex::new(None),
            })
        }

        fn events(&self) -> Vec<Event> {
            self.events.lock().unwrap().clone()
        }

        /// The lines of the log of every add-on, in order.
        fn log_lines(&self) -> Vec<(AddonLogLevel, String)> {
            self.events()
                .into_iter()
                .filter_map(|e| match e {
                    Event::Log(_, level, text) => Some((level, text)),
                    _ => None,
                })
                .collect()
        }

        fn texts(&self) -> Vec<String> {
            self.log_lines().into_iter().map(|(_, text)| text).collect()
        }

        /// Waits (at most five seconds) until the events are such that `what` says so.
        fn wait_for(&self, what: impl Fn(&[Event]) -> bool) -> bool {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if what(&self.events.lock().unwrap()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        }

        fn wait_for_text(&self, text: &str) -> bool {
            self.wait_for(|events| events.iter().any(|e| matches!(e, Event::Log(_, _, t) if t == text)))
        }

        /// Chooses an entry of an add-on in the menu of a device, as the user does; whether there was one.
        fn click(&self, addon: &str, id: &str, device: &str) -> bool {
            let item = MenuItemId::Addon { addon: addon.into(), id: id.into() };
            let handler = self.menu.lock().unwrap().handler(&item);
            handler.map(|h| h(device)).is_some()
        }

        /// Chooses an entry of an add-on in the menu of an app.
        fn click_app(&self, addon: &str, id: &str, device: &str, package: &str) -> bool {
            let item = AppMenuItemId::Addon { addon: addon.into(), id: id.into() };
            let handler = self.app_menu.lock().unwrap().handler(&item);
            handler.map(|h| h(device, package)).is_some()
        }

        /// Presses a button of an add-on on the toolbar.
        fn press(&self, addon: &str, id: &str, device: &str, app: Option<&str>) -> bool {
            let button = ButtonId::Addon { addon: addon.into(), id: id.into() };
            let handler = self.toolbar.lock().unwrap().handler(&button);
            handler.map(|h| h(device, app)).is_some()
        }
    }

    impl AddonHost for FakeHost {
        fn log(&self, addon: &str, level: AddonLogLevel, text: &str) {
            self.events.lock().unwrap().push(Event::Log(addon.into(), level, text.into()));
        }

        fn register_device_menu_item(
            &self,
            addon: &str,
            id: &str,
            label: &str,
            handler: MenuHandler,
        ) -> Result<(), MenuRegisterError> {
            self.menu.lock().unwrap().register(addon, id, label, handler)?;
            self.events.lock().unwrap().push(Event::Registered(addon.into(), id.into(), label.into()));
            Ok(())
        }

        fn register_app_menu_item(
            &self,
            addon: &str,
            id: &str,
            label: &str,
            handler: AppMenuHandler,
        ) -> Result<(), MenuRegisterError> {
            self.app_menu.lock().unwrap().register(addon, id, label, handler)?;
            self.events.lock().unwrap().push(Event::AppRegistered(addon.into(), id.into(), label.into()));
            Ok(())
        }

        fn register_toolbar_button(
            &self,
            addon: &str,
            id: &str,
            label: &str,
            handler: ButtonHandler,
        ) -> Result<(), MenuRegisterError> {
            self.toolbar.lock().unwrap().register(addon, id, label, handler)?;
            self.events.lock().unwrap().push(Event::ToolbarRegistered(addon.into(), id.into(), label.into()));
            Ok(())
        }

        fn remove_registrations(&self, addon: &str) {
            self.menu.lock().unwrap().remove_addon(addon);
            self.app_menu.lock().unwrap().remove_addon(addon);
            self.toolbar.lock().unwrap().remove_addon(addon);
            self.events.lock().unwrap().push(Event::Removed(addon.into()));
        }

        fn device_shell(&self, addon: &str, device: &str, command: &str) -> HostFuture<Result<String, CoreError>> {
            self.events.lock().unwrap().push(Event::Shell(addon.into(), device.into(), command.into()));
            let delay = *self.shell_delay.lock().unwrap();
            let result = self.shell_result.lock().unwrap().clone();
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                result
            })
        }

        fn spawn_process(
            &self,
            addon: &str,
            spec: ProcessSpec,
            listener: ProcessListener,
        ) -> Result<ProcessId, SpawnError> {
            (*self.spawn_result.lock().unwrap())?;
            let mut listeners = self.listeners.lock().unwrap();
            listeners.push(listener);
            self.events.lock().unwrap().push(Event::Spawned(addon.into(), spec.command, spec.args));
            Ok(ProcessId(listeners.len() as u32))
        }

        fn kill_process(&self, addon: &str, id: ProcessId) -> bool {
            self.events.lock().unwrap().push(Event::Killed(addon.into(), id));
            true
        }

        fn close_process_input(&self, addon: &str, id: ProcessId) -> bool {
            self.events.lock().unwrap().push(Event::InputClosed(addon.into(), id));
            true
        }

        fn pipe_screen(&self, addon: &str, device: &str, app: Option<&str>, id: ProcessId) -> Result<(), PipeError> {
            (*self.pipe_result.lock().unwrap())?;
            self.events.lock().unwrap().push(Event::Piped(addon.into(), device.into(), app.map(String::from), id));
            Ok(())
        }

        fn setting(&self, _addon: &str, key: &str) -> Option<SettingValue> {
            self.settings.lock().unwrap().get(key).cloned()
        }

        fn folder(&self, _addon: &str) -> Option<std::path::PathBuf> {
            self.folder.lock().unwrap().clone()
        }

        fn permission_denied(&self, addon: &str, permission: AddonPermission) {
            self.events.lock().unwrap().push(Event::Denied(addon.into(), permission));
        }
    }

    /// Short limits, so that the tests of a script that does not end are quick.
    fn limits() -> Limits {
        Limits {
            memory_bytes: 16 * 1024 * 1024,
            init_time: Duration::from_millis(500),
            call_time: Duration::from_millis(500),
            max_log_chars: 200,
            max_timers: 20,
            min_interval: Duration::from_millis(10),
            max_pending_events: 100,
        }
    }

    fn start_with(
        host: &Arc<FakeHost>,
        addon: &str,
        script: &str,
        granted: &[AddonPermission],
    ) -> Result<AddonRuntime, StartError> {
        AddonRuntime::start(addon, script, Granted::new(granted), host.clone(), limits())
    }

    /// Starts the add-on `demo`, which has to work (otherwise the test fails here).
    fn running(script: &str, granted: &[AddonPermission]) -> (Arc<FakeHost>, AddonRuntime) {
        let host = FakeHost::new();
        let started = start_with(&host, "demo", script, granted);
        assert!(started.is_ok(), "the add-on did not start: {:?}", started.as_ref().err());
        (host, started.unwrap())
    }

    /// An entry of the menu that runs `body` (a function of `device`, async) when it is chosen.
    fn entry(id: &str, body: &str) -> String {
        format!(r#"host.deviceMenu.register({{ id: "{id}", label: "Label {id}", onClick: async (device) => {{ {body} }} }});"#)
    }

    #[test]
    fn main_js_runs_when_the_add_on_starts_and_what_it_logs_reaches_the_log_with_its_name() {
        let (host, _runtime) = running(
            r#"host.log("hello"); console.log("a"); console.warn("b"); console.error("c"); host.log(42);"#,
            &[],
        );
        let expected = |level, text: &str| Event::Log("demo".into(), level, text.into());
        assert_eq!(
            host.events(),
            vec![
                expected(AddonLogLevel::Info, "hello"),
                expected(AddonLogLevel::Info, "a"),
                expected(AddonLogLevel::Warn, "b"),
                expected(AddonLogLevel::Error, "c"),
                expected(AddonLogLevel::Info, "42"),
            ]
        );
    }

    #[test]
    fn a_line_longer_than_the_limit_is_cut_and_says_so() {
        let (host, _runtime) = running(r#"host.log("é".repeat(250));"#, &[]);
        assert_eq!(host.texts(), vec![format!("{}…", "é".repeat(200))]);
        let (host, _runtime) = running(r#"host.log("x".repeat(200));"#, &[]);
        assert_eq!(host.texts(), vec!["x".repeat(200)]);
    }

    #[test]
    fn a_script_that_throws_or_does_not_parse_does_not_start_and_leaves_nothing_registered() {
        let registered_then_thrown = format!("{} throw new Error(\"boom\");", entry("go", "host.log(device);"));
        for script in ["throw new Error(\"boom\");", "let = ;", "host.nothing.here();", registered_then_thrown.as_str()] {
            let host = FakeHost::new();
            let started = start_with(&host, "demo", script, &[]);
            assert!(matches!(&started, Err(StartError::Script(text)) if !text.is_empty()), "{script}: {started:?}");
            assert!(!host.click("demo", "go", "dev-1"), "{script}: an entry was left");
        }
        // A failure after something was registered says that it was taken away again.
        let host = FakeHost::new();
        let _ = start_with(&host, "demo", &registered_then_thrown, &[]);
        assert_eq!(host.events().last(), Some(&Event::Removed("demo".into())));
    }

    #[test]
    fn a_main_js_that_never_ends_is_interrupted_and_is_a_timeout() {
        let host = FakeHost::new();
        let began = Instant::now();
        let started = start_with(&host, "demo", "while (true) {}", &[]);
        assert!(matches!(started, Err(StartError::Timeout)), "{started:?}");
        assert!(began.elapsed() < Duration::from_secs(4), "took {:?}", began.elapsed());
    }

    #[test]
    fn registering_an_entry_tells_the_host_and_choosing_it_runs_the_callback_with_the_device() {
        let (host, _runtime) = running(&entry("reboot", r#"host.log("clicked " + device);"#), &[]);
        assert_eq!(host.events(), vec![Event::Registered("demo".into(), "reboot".into(), "Label reboot".into())]);
        assert!(host.click("demo", "reboot", "dev-1"));
        assert!(host.wait_for_text("clicked dev-1"), "{:?}", host.events());
        assert!(host.click("demo", "reboot", "dev-2"));
        assert!(host.wait_for_text("clicked dev-2"));
    }

    #[test]
    fn what_the_host_refuses_to_register_is_an_error_in_the_script_with_a_typed_reason() {
        let script = r#"
            const attempt = (what, entry) => {
                try { host.deviceMenu.register(entry); host.log(what + " registered"); }
                catch (e) { host.log(what + " " + e.name + ":" + e.code); }
            };
            const ok = async () => {};
            attempt("first", { id: "one", label: "One", onClick: ok });
            attempt("again", { id: "one", label: "Other", onClick: ok });
            attempt("bad id", { id: "has space", label: "x", onClick: ok });
            attempt("bad label", { id: "two", label: "", onClick: ok });
            attempt("no callback", { id: "three", label: "Three", onClick: "not a function" });
            attempt("no entry", undefined);
        "#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(
            host.texts(),
            vec![
                "first registered",
                "again MenuRegisterError:already_registered",
                "bad id MenuRegisterError:invalid_id",
                "bad label MenuRegisterError:invalid_label",
                "no callback TypeError:undefined",
                "no entry TypeError:undefined",
            ]
        );
        // Only the first reached the host.
        let registered = host.events().iter().filter(|e| matches!(e, Event::Registered(..))).count();
        assert_eq!(registered, 1);
    }

    #[test]
    fn a_call_that_needs_a_permission_is_refused_without_it_and_the_host_is_told_and_not_asked() {
        let body = r#"
            try { const out = await host.deviceShell(device, "echo hi"); host.log("got " + out); }
            catch (e) { host.log(e.name + ":" + e.permission); }
        "#;
        let (host, _runtime) = running(&entry("sh", body), &[]);
        assert!(host.click("demo", "sh", "dev-1"));
        assert!(host.wait_for_text("PermissionDenied:device_shell"), "{:?}", host.events());
        let events = host.events();
        assert!(events.contains(&Event::Denied("demo".into(), SHELL)), "{events:?}");
        assert!(!events.iter().any(|e| matches!(e, Event::Shell(..))), "the host ran a command: {events:?}");
    }

    #[test]
    fn with_the_permission_the_command_goes_to_the_host_and_its_output_comes_back() {
        let body = r#"
            try { const out = await host.deviceShell(device, "echo hi"); host.log("got " + out); }
            catch (e) { host.log(e.name + ":" + e.code); }
        "#;
        let (host, _runtime) = running(&entry("sh", body), &[SHELL]);
        assert!(host.click("demo", "sh", "dev-1"));
        assert!(host.wait_for_text("got shell output"), "{:?}", host.events());
        assert!(host.events().contains(&Event::Shell("demo".into(), "dev-1".into(), "echo hi".into())));
        // What the host answers with an error is an error in the script with the code of the core.
        *host.shell_result.lock().unwrap() = Err(CoreError::NotOnline);
        assert!(host.click("demo", "sh", "dev-1"));
        assert!(host.wait_for_text("DeviceError:not_online"), "{:?}", host.events());
    }

    #[test]
    fn an_error_in_a_callback_is_logged_as_an_error_and_the_next_choice_still_works() {
        let body = r#"
            globalThis.count = (globalThis.count || 0) + 1;
            if (count === 1) throw new Error("first time fails");
            host.log("second time " + device);
        "#;
        let (host, _runtime) = running(&entry("flaky", body), &[]);
        assert!(host.click("demo", "flaky", "dev-1"));
        assert!(host.wait_for(|e| e.iter().any(|e| matches!(e, Event::Log(_, AddonLogLevel::Error, t) if t.contains("first time fails")))));
        assert!(host.click("demo", "flaky", "dev-2"));
        assert!(host.wait_for_text("second time dev-2"), "{:?}", host.events());
    }

    #[test]
    fn a_callback_that_never_ends_or_eats_all_the_memory_is_stopped_and_the_add_on_keeps_working() {
        let body = r#"
            globalThis.count = (globalThis.count || 0) + 1;
            if (count === 1) { while (true) {} }
            if (count === 2) { const a = []; while (true) { a.push(new Array(100000).fill(1)); } }
            host.log("third time " + device);
        "#;
        let (host, _runtime) = running(&entry("hog", body), &[]);
        for device in ["dev-1", "dev-2", "dev-3"] {
            assert!(host.click("demo", "hog", device));
        }
        assert!(host.wait_for_text("third time dev-3"), "{:?}", host.events());
        let errors = host.log_lines().iter().filter(|(level, _)| *level == AddonLogLevel::Error).count();
        assert_eq!(errors, 2, "{:?}", host.events());
    }

    #[test]
    fn the_script_has_no_way_out_of_what_the_host_gives_it() {
        let script = r#"host.log([
            typeof require, typeof process, typeof fetch, typeof XMLHttpRequest, typeof WebSocket,
            typeof std, typeof os, typeof Buffer,
            typeof host, typeof console,
            typeof setTimeout, typeof clearTimeout, typeof setInterval, typeof clearInterval,
        ].join(","));"#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(
            host.texts(),
            vec!["undefined,undefined,undefined,undefined,undefined,undefined,undefined,undefined,object,object,function,function,function,function"]
        );
    }

    #[test]
    fn a_timer_runs_after_its_delay_in_the_order_of_the_delays_and_one_that_was_cleared_does_not() {
        let script = r#"
            setTimeout(() => host.log("b 60"), 60);
            setTimeout(() => host.log("a 10"), 10);
            const cleared = setTimeout(() => host.log("never"), 30);
            clearTimeout(cleared);
            host.log(typeof cleared);
        "#;
        let (host, _runtime) = running(script, &[]);
        assert!(host.wait_for_text("b 60"), "{:?}", host.events());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(host.texts(), vec!["number", "a 10", "b 60"]);
    }

    #[test]
    fn an_interval_runs_again_and_again_until_it_is_cleared() {
        let script = r#"
            let n = 0;
            const id = setInterval(() => { n++; host.log("tick " + n); if (n === 3) clearInterval(id); }, 10);
        "#;
        let (host, _runtime) = running(script, &[]);
        assert!(host.wait_for_text("tick 3"), "{:?}", host.events());
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(host.texts(), vec!["tick 1", "tick 2", "tick 3"]);
    }

    #[test]
    fn an_interval_shorter_than_the_least_is_made_that_long() {
        // With no least time this would run thousands of times in the half second.
        let script = r#"
            let n = 0;
            setInterval(() => { n++; }, 0);
            setTimeout(() => host.log("ran " + (n <= 60 ? "at most 60" : "too often: " + n)), 500);
        "#;
        let (host, _runtime) = running(script, &[]);
        assert!(host.wait_for(|e| e.iter().any(|e| matches!(e, Event::Log(_, _, t) if t.starts_with("ran ")))));
        assert_eq!(host.texts(), vec!["ran at most 60"]);
    }

    #[test]
    fn a_timer_that_fails_is_logged_and_the_others_still_run() {
        let script = r#"
            setTimeout(() => { throw new Error("timer failed"); }, 10);
            setTimeout(() => host.log("after"), 40);
        "#;
        let (host, _runtime) = running(script, &[]);
        assert!(host.wait_for_text("after"), "{:?}", host.events());
        assert!(host.log_lines().iter().any(|(level, text)| *level == AddonLogLevel::Error && text.contains("timer failed")));
    }

    #[test]
    fn timers_do_not_outlive_the_add_on() {
        let (host, runtime) = running(r#"setTimeout(() => host.log("late"), 150);"#, &[]);
        runtime.stop();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(host.texts(), Vec::<String>::new());
    }

    #[test]
    fn there_is_a_limit_of_timers_and_a_timer_needs_a_function() {
        let script = r#"
            for (let i = 0; i < 25; i++) {
                try { setTimeout(() => {}, 100000); }
                catch (e) { host.log("timer " + (i + 1) + " " + e.name); break; }
            }
            try { setTimeout("not a function", 10); } catch (e) { host.log("text " + e.name); }
            try { setInterval(undefined, 10); } catch (e) { host.log("undefined " + e.name); }
        "#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(host.texts(), vec!["timer 21 TimerLimit", "text TypeError", "undefined TypeError"]);
    }

    #[test]
    fn two_add_ons_share_nothing() {
        let host = FakeHost::new();
        let a = start_with(&host, "first", r#"globalThis.shared = 1; host.log("a " + typeof shared);"#, &[]);
        let b = start_with(&host, "second", r#"host.log("b " + typeof shared);"#, &[]);
        assert!(a.is_ok() && b.is_ok(), "{a:?} {b:?}");
        let lines: Vec<(String, String)> = host
            .events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Log(addon, _, text) => Some((addon, text)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lines,
            vec![("first".to_string(), "a number".to_string()), ("second".to_string(), "b undefined".to_string())]
        );
    }

    #[test]
    fn the_callbacks_of_one_add_on_run_one_at_a_time_in_the_order_they_came() {
        let body = r#"
            globalThis.n = (globalThis.n || 0) + 1;
            const k = n;
            host.log("start " + k);
            await host.deviceShell(device, "x");
            host.log("end " + k);
        "#;
        let (host, _runtime) = running(&entry("go", body), &[SHELL]);
        *host.shell_delay.lock().unwrap() = Duration::from_millis(100);
        assert!(host.click("demo", "go", "dev-1"));
        assert!(host.click("demo", "go", "dev-2"));
        assert!(host.wait_for_text("end 2"), "{:?}", host.events());
        assert_eq!(host.texts(), vec!["start 1", "end 1", "start 2", "end 2"]);
    }

    #[test]
    fn work_that_main_js_started_goes_on_after_it_returned_and_its_failure_is_logged() {
        let script = r#"
            (async () => { const out = await host.deviceShell("dev-9", "uname"); host.log("init got " + out); })();
        "#;
        let host = FakeHost::new();
        *host.shell_result.lock().unwrap() = Ok("Linux".into());
        let started = start_with(&host, "demo", script, &[SHELL]);
        assert!(started.is_ok(), "{started:?}");
        assert!(host.wait_for_text("init got Linux"), "{:?}", host.events());

        let host = FakeHost::new();
        let started = start_with(&host, "demo", r#"(async () => { throw new Error("late failure"); })();"#, &[]);
        assert!(started.is_ok(), "{started:?}");
        assert!(
            host.wait_for(|e| e.iter().any(|e| matches!(e, Event::Log(_, AddonLogLevel::Error, t) if t.contains("late failure")))),
            "{:?}",
            host.events()
        );
    }

    #[test]
    fn stopping_takes_away_what_was_registered_and_returns_even_when_a_callback_does_not_end() {
        let (host, runtime) = running(&entry("hang", "while (true) {}"), &[]);
        assert!(host.click("demo", "hang", "dev-1"));
        // Let the callback begin.
        std::thread::sleep(Duration::from_millis(50));
        let began = Instant::now();
        runtime.stop();
        assert!(began.elapsed() < Duration::from_secs(3), "stopping took {:?}", began.elapsed());
        assert_eq!(host.events().last(), Some(&Event::Removed("demo".into())));
        assert!(!host.click("demo", "hang", "dev-1"), "the entry is still there");
    }

    // ---- the other places: the menu of an app and the toolbar

    #[test]
    fn an_entry_of_the_menu_of_an_app_tells_the_host_and_choosing_it_runs_the_callback_with_the_device_and_the_app() {
        let script = r#"host.appMenu.register({ id: "open-it", label: "Open it", onClick: async (device, app) => host.log("chosen " + device + " " + app) });"#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(host.events(), vec![Event::AppRegistered("demo".into(), "open-it".into(), "Open it".into())]);
        assert!(host.click_app("demo", "open-it", "dev-1", "com.example.app"));
        assert!(host.wait_for_text("chosen dev-1 com.example.app"), "{:?}", host.events());
    }

    #[test]
    fn a_button_of_the_toolbar_tells_the_host_and_pressing_it_runs_the_callback_with_the_device_and_the_app_or_none() {
        let script = r#"host.toolbar.register({ id: "tap", label: "Tap", onClick: async (device, app) => host.log("pressed " + device + " " + app) });"#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(host.events(), vec![Event::ToolbarRegistered("demo".into(), "tap".into(), "Tap".into())]);
        assert!(host.press("demo", "tap", "dev-1", None));
        assert!(host.wait_for_text("pressed dev-1 null"), "{:?}", host.events());
        assert!(host.press("demo", "tap", "dev-2", Some("com.example.app")));
        assert!(host.wait_for_text("pressed dev-2 com.example.app"), "{:?}", host.events());
    }

    #[test]
    fn what_the_host_refuses_for_the_menu_of_an_app_and_the_toolbar_is_an_error_with_a_typed_reason() {
        let script = r#"
            const attempt = (what, place, entry) => {
                try { host[place].register(entry); host.log(what + " registered"); }
                catch (e) { host.log(what + " " + e.name + ":" + e.code); }
            };
            const ok = async () => {};
            for (const place of ["appMenu", "toolbar"]) {
                attempt(place + " first", place, { id: "one", label: "One", onClick: ok });
                attempt(place + " again", place, { id: "one", label: "Other", onClick: ok });
                attempt(place + " bad id", place, { id: "has space", label: "x", onClick: ok });
                attempt(place + " bad label", place, { id: "two", label: "", onClick: ok });
                attempt(place + " no callback", place, { id: "three", label: "Three", onClick: "not a function" });
                attempt(place + " no entry", place, undefined);
            }
        "#;
        let (host, _runtime) = running(script, &[]);
        let mut expected = Vec::new();
        for place in ["appMenu", "toolbar"] {
            expected.extend([
                format!("{place} first registered"),
                format!("{place} again MenuRegisterError:already_registered"),
                format!("{place} bad id MenuRegisterError:invalid_id"),
                format!("{place} bad label MenuRegisterError:invalid_label"),
                format!("{place} no callback TypeError:undefined"),
                format!("{place} no entry TypeError:undefined"),
            ]);
        }
        assert_eq!(host.texts(), expected);
    }

    #[test]
    fn each_place_has_names_of_its_own_so_the_same_id_may_be_used_in_all_and_stopping_takes_them_all_away() {
        let script = r#"
            for (const place of ["deviceMenu", "appMenu", "toolbar"]) {
                host[place].register({ id: "go", label: "Go", onClick: async () => host.log("ran " + place) });
            }
        "#;
        let (host, runtime) = running(script, &[]);
        assert!(host.click("demo", "go", "d"));
        assert!(host.click_app("demo", "go", "d", "a.b"));
        assert!(host.press("demo", "go", "d", None));
        assert!(host.wait_for(|e| e.iter().filter(|e| matches!(e, Event::Log(..))).count() == 3), "{:?}", host.events());
        let mut ran = host.texts();
        ran.sort();
        assert_eq!(ran, vec!["ran appMenu", "ran deviceMenu", "ran toolbar"]);
        runtime.stop();
        assert!(!host.click("demo", "go", "d") && !host.click_app("demo", "go", "d", "a.b") && !host.press("demo", "go", "d", None));
    }

    // ---- events

    fn online(device: &str) -> AddonEvent {
        AddonEvent::DeviceOnline { device: device.into(), name: "Phone".into() }
    }

    #[test]
    fn an_event_runs_the_callbacks_of_the_add_on_with_the_members_of_that_event() {
        let script = r#"
            for (const name of ["device.online", "device.offline", "device.added", "screen.started", "screen.ended", "files.sent", "files.opened"]) {
                host.on(name, (e) => host.log(name + " " + JSON.stringify(e)));
            }
        "#;
        let (host, runtime) = running(script, &[]);
        let all = [
            (online("d1"), r#"device.online {"device":"d1","name":"Phone"}"#),
            (AddonEvent::DeviceOffline { device: "d1".into(), name: "Phone".into() }, r#"device.offline {"device":"d1","name":"Phone"}"#),
            (AddonEvent::DeviceAdded { device: "d1".into(), name: "Phone".into() }, r#"device.added {"device":"d1","name":"Phone"}"#),
            (AddonEvent::ScreenStarted { device: "d1".into(), app: None }, r#"screen.started {"app":null,"device":"d1"}"#),
            (
                AddonEvent::ScreenEnded { device: "d1".into(), app: Some("com.example.app".into()) },
                r#"screen.ended {"app":"com.example.app","device":"d1"}"#,
            ),
            (
                AddonEvent::FilesSent { device: "d1".into(), name: "a.txt".into(), error: None },
                r#"files.sent {"device":"d1","error":null,"name":"a.txt","ok":true}"#,
            ),
            (
                AddonEvent::FilesOpened { device: "d1".into(), file: "b.pdf".into(), error: Some("no_app".into()) },
                r#"files.opened {"device":"d1","error":"no_app","file":"b.pdf","ok":false}"#,
            ),
        ];
        for (event, _) in &all {
            runtime.emit(event.clone());
        }
        let expected: Vec<String> = all.iter().map(|(_, text)| text.to_string()).collect();
        assert!(host.wait_for_text(&expected[6]), "{:?}", host.events());
        assert_eq!(host.texts(), expected);
    }

    #[test]
    fn the_callbacks_of_an_event_run_in_the_order_they_were_subscribed_and_a_cancelled_one_does_not_run() {
        let script = format!(
            r#"
            const offA = host.on("device.online", (e) => host.log("a " + e.device));
            host.on("device.online", (e) => host.log("b " + e.device));
            host.on("device.offline", (e) => host.log("offline " + e.device));
            host.log(typeof offA);
            {}
            "#,
            entry("stop", "offA(); offA();")
        );
        let (host, runtime) = running(&script, &[]);
        runtime.emit(online("d1"));
        assert!(host.wait_for_text("b d1"), "{:?}", host.events());
        // Cancelling twice is no error, and cancels only that one.
        assert!(host.click("demo", "stop", "dev-1"));
        std::thread::sleep(Duration::from_millis(100));
        runtime.emit(online("d2"));
        assert!(host.wait_for_text("b d2"), "{:?}", host.events());
        assert_eq!(host.texts(), vec!["function", "a d1", "b d1", "b d2"]);
    }

    #[test]
    fn a_subscription_that_cannot_be_made_is_an_error_in_the_script_with_a_typed_reason() {
        let script = r#"
            const attempt = (what, name, callback) => {
                try { host.on(name, callback); host.log(what + " subscribed"); }
                catch (e) { host.log(what + " " + e.name + ":" + e.event); }
            };
            attempt("unknown", "screen.stop", () => {});
            attempt("no name", 7, () => {});
            attempt("no callback", "device.online", "not a function");
            attempt("good", "device.online", () => {});
        "#;
        let (host, _runtime) = running(script, &[]);
        assert_eq!(
            host.texts(),
            vec!["unknown UnknownEvent:screen.stop", "no name TypeError:undefined", "no callback TypeError:undefined", "good subscribed"]
        );
    }

    #[test]
    fn a_callback_of_an_event_that_fails_is_logged_with_the_event_and_the_others_and_the_next_event_still_run() {
        let script = r#"
            host.on("device.online", (e) => { throw new Error("first fails for " + e.device); });
            host.on("device.online", (e) => host.log("second " + e.device));
        "#;
        let (host, runtime) = running(script, &[]);
        runtime.emit(online("d1"));
        runtime.emit(online("d2"));
        assert!(host.wait_for_text("second d2"), "{:?}", host.events());
        let errors: Vec<String> = host
            .log_lines()
            .into_iter()
            .filter(|(level, _)| *level == AddonLogLevel::Error)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("device.online") && errors[0].contains("first fails for d1"), "{errors:?}");
        assert!(errors[1].contains("first fails for d2"), "{errors:?}");
        assert_eq!(host.texts().iter().filter(|t| t.starts_with("second")).count(), 2);
    }

    #[test]
    fn when_too_many_events_wait_for_a_busy_add_on_the_oldest_are_dropped_and_the_log_says_how_many() {
        let script = r#"
            host.on("device.online", async (e) => {
                host.log("start " + e.device);
                await host.deviceShell(e.device, "x");
            });
        "#;
        let host = FakeHost::new();
        let started =
            AddonRuntime::start("demo", script, Granted::new(&[SHELL]), host.clone(), Limits { max_pending_events: 3, ..limits() });
        assert!(started.is_ok(), "the add-on did not start: {:?}", started.as_ref().err());
        let runtime = started.unwrap();
        *host.shell_delay.lock().unwrap() = Duration::from_millis(300);
        runtime.emit(online("d1"));
        assert!(host.wait_for_text("start d1"), "{:?}", host.events());
        // The add-on is busy with the first; the limit (3) leaves the newest three of the nine that come.
        for n in 2..=10 {
            runtime.emit(online(&format!("d{n}")));
        }
        assert!(host.wait_for_text("start d10"), "{:?}", host.events());
        let starts: Vec<String> = host.texts().into_iter().filter(|t| t.starts_with("start")).collect();
        assert_eq!(starts, vec!["start d1", "start d8", "start d9", "start d10"]);
        let warnings: Vec<String> = host
            .log_lines()
            .into_iter()
            .filter(|(level, _)| *level == AddonLogLevel::Warn)
            .map(|(_, text)| text)
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains('6'), "the number dropped is told: {warnings:?}");
    }

    #[test]
    fn an_event_that_nobody_subscribed_to_changes_nothing_and_the_add_on_goes_on() {
        let (host, runtime) = running(&entry("go", r#"host.log("clicked " + device);"#), &[]);
        runtime.emit(online("d1"));
        runtime.emit(AddonEvent::ScreenEnded { device: "d1".into(), app: None });
        assert!(host.click("demo", "go", "dev-1"));
        assert!(host.wait_for_text("clicked dev-1"), "{:?}", host.events());
        assert_eq!(host.texts(), vec!["clicked dev-1"]);
    }

    #[test]
    fn an_add_on_that_is_stopped_while_idle_stops_at_once() {
        let (host, runtime) = running(&entry("idle", r#"host.log("x");"#), &[]);
        let began = Instant::now();
        runtime.stop();
        assert!(began.elapsed() < Duration::from_secs(2), "stopping took {:?}", began.elapsed());
        assert!(host.events().contains(&Event::Removed("demo".into())));
    }

    #[test]
    fn the_functions_of_host_are_exactly_the_ones_the_api_lists() {
        // The path of every function in `host` (one level of objects below it), as a script reads them.
        let script = r#"
            const paths = [];
            for (const [name, member] of Object.entries(host)) {
                if (typeof member === "function") { paths.push(name); continue; }
                for (const [inner, value] of Object.entries(member)) {
                    if (typeof value === "function") paths.push(name + "." + inner);
                }
            }
            host.log(JSON.stringify(paths.sort()));"#;
        let (host, _runtime) = running(script, &[]);
        let listed = crate::core::addons::api::host_members();
        assert_eq!(host.texts(), vec![serde_json::to_string(&listed).unwrap()]);
    }

    const RUN: AddonPermission = AddonPermission::RunProcess;
    const SCREEN: AddonPermission = AddonPermission::ScreenCapture;

    #[test]
    fn a_program_is_started_with_the_command_and_arguments_as_they_are_and_its_lines_and_end_reach_the_callbacks() {
        let script = r#"
            host.process.spawn({
                command: "my tool",
                args: ["-u", "a b", "--x=1;rm -rf"],
                onLine: (line) => host.log("line " + line),
                onExit: (code) => host.log("exit " + code),
            });"#;
        let (host, _runtime) = running(script, &[RUN]);
        let args = vec!["-u".to_string(), "a b".to_string(), "--x=1;rm -rf".to_string()];
        assert_eq!(host.events(), vec![Event::Spawned("demo".into(), "my tool".into(), args)]);
        host.tell(0, ProcessEvent::Line("one".into()));
        host.tell(0, ProcessEvent::Line("two".into()));
        host.tell(0, ProcessEvent::Exited(Some(0)));
        assert!(host.wait_for_text("exit 0"), "{:?}", host.events());
        assert_eq!(host.texts(), vec!["line one", "line two", "exit 0"]);
        // A program that ended without a code says so with null, and one that was given no callbacks is fine.
        let script = r#"host.process.spawn({ command: "a", onExit: (code) => host.log("exit " + code) });
                        host.process.spawn({ command: "b" });"#;
        let (host, _runtime) = running(script, &[RUN]);
        host.tell(0, ProcessEvent::Exited(None));
        host.tell(1, ProcessEvent::Line("nobody listens".into()));
        host.tell(1, ProcessEvent::Exited(Some(1)));
        assert!(host.wait_for_text("exit null"), "{:?}", host.events());
        assert_eq!(host.texts(), vec!["exit null"]);
    }

    #[test]
    fn a_callback_of_a_program_that_throws_is_logged_and_the_lines_after_it_still_come() {
        let script = r#"host.process.spawn({ command: "a", onLine: (line) => {
            if (line === "bad") throw new Error("boom");
            host.log("line " + line);
        } });"#;
        let (host, _runtime) = running(script, &[RUN]);
        for line in ["bad", "good"] {
            host.tell(0, ProcessEvent::Line(line.into()));
        }
        assert!(host.wait_for_text("line good"), "{:?}", host.events());
        let errors: Vec<_> = host.log_lines().into_iter().filter(|(level, _)| *level == AddonLogLevel::Error).collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].1.contains("boom"), "{errors:?}");
    }

    #[test]
    fn starting_a_program_needs_the_permission_and_the_host_is_not_asked_without_it() {
        let script = r#"try { host.process.spawn({ command: "a" }); host.log("started"); }
                        catch (e) { host.log(e.name + " " + e.permission); }"#;
        let (host, _runtime) = running(script, &[SHELL, SCREEN]);
        assert_eq!(host.texts(), vec!["PermissionDenied run_process"]);
        assert!(host.events().contains(&Event::Denied("demo".into(), RUN)));
        assert!(!host.events().iter().any(|e| matches!(e, Event::Spawned(..))));
    }

    #[test]
    fn a_program_the_host_refuses_throws_an_error_with_the_reason_as_its_code() {
        let script = r#"try { host.process.spawn({ command: "a" }); host.log("started"); }
                        catch (e) { host.log(e.name + " " + e.code); }"#;
        let cases = [
            (SpawnError::InvalidCommand, "invalid_command"),
            (SpawnError::NotFound, "not_found"),
            (SpawnError::NotAllowed, "not_allowed"),
            (SpawnError::TooMany, "too_many"),
            (SpawnError::Failed, "failed"),
        ];
        for (error, code) in cases {
            let host = FakeHost::new();
            *host.spawn_result.lock().unwrap() = Err(error);
            let started = start_with(&host, "demo", script, &[RUN]);
            assert!(started.is_ok(), "{error:?}");
            assert_eq!(host.texts(), vec![format!("ProcessError {code}")], "{error:?}");
        }
    }

    #[test]
    fn what_is_not_a_command_and_arguments_is_refused_before_the_host_is_asked() {
        let script = r#"
            const bad = [
                () => host.process.spawn(42),
                () => host.process.spawn({ command: 1 }),
                () => host.process.spawn({ command: "a", args: "x" }),
                () => host.process.spawn({ command: "a", args: [1] }),
                () => host.process.spawn({ command: "a", onLine: "not a function" }),
            ];
            for (const call of bad) { try { call(); host.log("accepted"); } catch (e) { host.log(e.name); } }"#;
        let (host, _runtime) = running(script, &[RUN]);
        assert_eq!(host.texts(), vec!["TypeError"; 5]);
        assert!(!host.events().iter().any(|e| matches!(e, Event::Spawned(..))));
    }

    #[test]
    fn the_handle_of_a_program_ends_it_and_closes_its_input_and_says_what_the_host_said() {
        let script = r#"const p = host.process.spawn({ command: "a" });
                        host.log("closed " + p.closeInput());
                        host.log("killed " + p.kill());"#;
        let (host, _runtime) = running(script, &[RUN]);
        assert_eq!(host.texts(), vec!["closed true", "killed true"]);
        let events = host.events();
        assert!(events.contains(&Event::InputClosed("demo".into(), ProcessId(1))), "{events:?}");
        assert!(events.contains(&Event::Killed("demo".into(), ProcessId(1))), "{events:?}");
    }

    #[test]
    fn a_screen_is_fed_to_a_program_of_the_add_on_with_the_permission_for_the_screen() {
        let script = r#"const p = host.process.spawn({ command: "a" });
                        host.screen.pipeTo(p, "dev-1");
                        host.log("piped");"#;
        let (host, _runtime) = running(script, &[RUN, SCREEN]);
        assert_eq!(host.texts(), vec!["piped"]);
        assert!(host.events().contains(&Event::Piped("demo".into(), "dev-1".into(), None, ProcessId(1))));
        // The permission to run programs is not the one for the screen.
        let script = r#"const p = host.process.spawn({ command: "a" });
                        try { host.screen.pipeTo(p, "dev-1"); host.log("piped"); }
                        catch (e) { host.log(e.name + " " + e.permission); }"#;
        let (host, _runtime) = running(script, &[RUN]);
        assert_eq!(host.texts(), vec!["PermissionDenied screen_capture"]);
        assert!(host.events().contains(&Event::Denied("demo".into(), SCREEN)));
        assert!(!host.events().iter().any(|e| matches!(e, Event::Piped(..))));
    }

    #[test]
    fn a_screen_that_cannot_be_followed_throws_an_error_with_the_reason_and_only_a_real_handle_is_taken() {
        let script = r#"const p = host.process.spawn({ command: "a" });
                        for (const target of [p, { id: 1 }, 1, null, "1"]) {
                            try { host.screen.pipeTo(target, "dev-1"); host.log("piped"); }
                            catch (e) { host.log(e.name + " " + (e.code ?? "")); }
                        }"#;
        let host = FakeHost::new();
        *host.pipe_result.lock().unwrap() = Err(PipeError::NoScreen);
        assert!(start_with(&host, "demo", script, &[RUN, SCREEN]).is_ok());
        assert_eq!(host.texts(), vec!["ScreenError no_screen", "TypeError ", "TypeError ", "TypeError ", "TypeError "]);
        let host = FakeHost::new();
        *host.pipe_result.lock().unwrap() = Err(PipeError::NoProcess);
        assert!(start_with(&host, "demo", script, &[RUN, SCREEN]).is_ok());
        assert_eq!(host.texts()[0], "ScreenError no_process");
        assert!(!host.events().iter().any(|e| matches!(e, Event::Piped(..))));
    }

    #[test]
    fn the_window_of_an_app_is_followed_by_giving_its_package_and_the_screen_of_the_device_by_giving_none() {
        let script = r#"const p = host.process.spawn({ command: "a" });
                        host.screen.pipeTo(p, "dev-1", "com.example.app");
                        host.screen.pipeTo(p, "dev-1", null);
                        host.screen.pipeTo(p, "dev-1", undefined);
                        host.screen.pipeTo(p, "dev-1");"#;
        let (host, _runtime) = running(script, &[RUN, SCREEN]);
        let piped: Vec<Option<String>> = host
            .events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Piped(_, _, app, _) => Some(app),
                _ => None,
            })
            .collect();
        assert_eq!(piped, vec![Some("com.example.app".to_string()), None, None, None]);
    }

    #[test]
    fn the_script_reads_the_settings_the_user_set_by_their_names_and_one_that_is_not_declared_is_undefined() {
        let host = FakeHost::new();
        {
            let mut settings = host.settings.lock().unwrap();
            settings.insert("level".into(), SettingValue::Number(7));
            settings.insert("name".into(), SettingValue::Text("abc".into()));
            settings.insert("on".into(), SettingValue::Boolean(false));
        }
        let script = r#"
            host.log(JSON.stringify([host.settings.get("level"), host.settings.get("name"), host.settings.get("on")]));
            host.log(typeof host.settings.get("nothing"));
            try { host.settings.get(5); host.log("accepted"); } catch (e) { host.log(e.name); }"#;
        let started = start_with(&host, "demo", script, &[]);
        assert!(started.is_ok(), "{:?}", started.as_ref().err());
        assert_eq!(host.texts(), vec![r#"[7,"abc",false]"#, "undefined", "TypeError"]);
    }

    #[test]
    fn the_script_asks_the_folder_it_is_installed_in_and_gets_the_path_as_the_program_has_it() {
        let host = FakeHost::new();
        // A path made up for the test; the form of a path is the one of the system the test runs on.
        let folder = std::env::temp_dir().join("made-up-addons").join("tuned");
        *host.folder.lock().unwrap() = Some(folder.clone());
        let started = start_with(&host, "tuned", r#"host.log(host.addon.folder()); host.log(typeof host.addon.folder());"#, &[]);
        assert!(started.is_ok(), "{:?}", started.as_ref().err());
        assert_eq!(host.texts(), vec![folder.to_str().unwrap().to_string(), "string".to_string()]);
    }

    #[test]
    fn an_add_on_the_program_does_not_have_installed_gets_an_error_and_not_an_empty_path() {
        let host = FakeHost::new();
        let script = r#"try { host.addon.folder(); host.log("accepted"); } catch (e) { host.log(e.name); }"#;
        let started = start_with(&host, "gone", script, &[]);
        assert!(started.is_ok(), "{:?}", started.as_ref().err());
        assert_eq!(host.texts(), vec!["AddonFolderError"]);
    }

    #[test]
    fn a_change_of_a_setting_reaches_the_subscription_with_the_key_and_the_new_value_and_a_failing_one_does_not_stop_the_rest() {
        let script = r#"
            host.on("settings.changed", ({ key, value }) => { if (key === "bad") throw new Error("boom"); host.log(key + "=" + value); });
            host.on("settings.changed", ({ key, value }) => host.log("second " + typeof value));"#;
        let (host, runtime) = running(script, &[]);
        runtime.emit(AddonEvent::SettingChanged { key: "level".into(), value: SettingValue::Number(9) });
        runtime.emit(AddonEvent::SettingChanged { key: "bad".into(), value: SettingValue::Boolean(true) });
        runtime.emit(AddonEvent::SettingChanged { key: "name".into(), value: SettingValue::Text("x".into()) });
        assert!(host.wait_for_text("second string"), "{:?}", host.events());
        let infos: Vec<String> = host
            .log_lines()
            .into_iter()
            .filter(|(level, _)| *level == AddonLogLevel::Info)
            .map(|(_, text)| text)
            .filter(|text| !text.starts_with("second"))
            .collect();
        assert_eq!(infos, vec!["level=9", "name=x"]);
        let errors: Vec<_> = host.log_lines().into_iter().filter(|(level, _)| *level == AddonLogLevel::Error).collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
    }

    #[test]
    fn an_app_that_is_not_text_is_refused_before_the_host_is_asked() {
        let script = r#"const p = host.process.spawn({ command: "a" });
                        for (const app of [42, {}, ["com.example.app"], true]) {
                            try { host.screen.pipeTo(p, "dev-1", app); host.log("piped"); }
                            catch (e) { host.log(e.name); }
                        }"#;
        let (host, _runtime) = running(script, &[RUN, SCREEN]);
        assert_eq!(host.texts(), vec!["TypeError"; 4]);
        assert!(!host.events().iter().any(|e| matches!(e, Event::Piped(..))));
    }
}
