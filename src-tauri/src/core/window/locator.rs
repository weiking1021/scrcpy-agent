//! Finding a window of another program on screen, to attach the toolbar to it.
//!
//! The logic that uses this (following the window, closing the toolbar with
//! it) only talks to the `WindowLocator` trait, so it is tested with a fake
//! on every system. Each system has its own implementation behind the trait,
//! and every implementation must pass the same contract tests (see the tests
//! below).

use super::layout::Rect;
use std::time::Duration;

/// A window as found on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInfo {
    /// Where the window is, in physical screen pixels. This is the visible
    /// window, without the invisible resize border some systems add. It means
    /// nothing while `minimized` is true.
    ///
    /// The values are what the system reports at that moment. While a window
    /// is being restored, Windows needs about 80 ms before the position
    /// catches up: `minimized` is already false and `rect` is still the
    /// parking place of minimised windows, far outside every screen. Whoever
    /// uses this has to cope with a rectangle that is on no screen.
    pub rect: Rect,
    pub minimized: bool,
}

pub trait WindowLocator {
    /// The top-level window whose title is exactly `title` (the comparison
    /// is case sensitive and the whole title must match), or `None` if there
    /// is no such window.
    fn find_by_title(&self, title: &str) -> Option<WindowInfo>;
}

/// Tells when a window may have changed, so nobody has to look at it over and
/// over. Following a window by asking for its position ten times a second
/// makes whatever follows it visibly lag behind; the window system itself
/// says when a window moves, and this passes that on.
pub trait WindowWatcher {
    /// Waits until the window with the watched title may have changed (moved,
    /// resized, minimised, restored), or until `timeout` has passed. Returns
    /// true when a change woke it, false when the time ran out. It is only
    /// woken by the watched window; other windows moving do not count.
    fn wait_for_change(&self, timeout: Duration) -> bool;
}

/// The watcher for Windows. It asks Windows to report every change of
/// position or size (`EVENT_OBJECT_LOCATIONCHANGE`) to a thread of its own,
/// and wakes whoever is waiting when that window is the watched one.
#[cfg(windows)]
pub struct WindowsWatcher(windows_impl::Watcher);

#[cfg(windows)]
impl WindowsWatcher {
    /// Watches the window whose title is exactly `title` (it may not exist yet).
    /// Changes of windows of this process are not reported.
    pub fn new(title: &str) -> Self {
        WindowsWatcher(windows_impl::Watcher::start(title))
    }
}

#[cfg(windows)]
impl WindowWatcher for WindowsWatcher {
    fn wait_for_change(&self, timeout: Duration) -> bool {
        self.0.wait_for_change(timeout)
    }
}

/// The handle of the window with this title, for acting on that window (for
/// example making it the owner of another one).
#[cfg(windows)]
pub fn window_handle(title: &str) -> Option<isize> {
    windows_impl::handle_of(title)
}

/// The locator for Windows. It remembers the window it found last, so the
/// next look is one question about that window instead of a walk through
/// every window on the desktop.
#[cfg(windows)]
pub struct WindowsLocator {
    last_found: std::cell::RefCell<Option<windows_impl::Remembered>>,
}

#[cfg(windows)]
impl WindowsLocator {
    pub fn new() -> Self {
        WindowsLocator { last_found: std::cell::RefCell::new(None) }
    }
}

#[cfg(windows)]
impl Default for WindowsLocator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl WindowLocator for WindowsLocator {
    fn find_by_title(&self, title: &str) -> Option<WindowInfo> {
        windows_impl::find_by_title(title, &mut self.last_found.borrow_mut())
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{Rect, WindowInfo};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, EnumWindows, GetMessageW, GetWindowRect,
        GetWindowTextLengthW, GetWindowTextW, IsIconic, IsWindow, MSG, OBJID_WINDOW, PM_NOREMOVE,
        PeekMessageW, PostThreadMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT,
        WINEVENT_SKIPOWNPROCESS, WM_QUIT,
    };

    /// The window found last: the title that was asked for (as the UTF-16
    /// Windows stores titles in) and the handle of the window.
    pub type Remembered = (Vec<u16>, usize);

    struct Search {
        wanted: Vec<u16>,
        found: Option<(usize, WindowInfo)>,
    }

    pub fn find_by_title(title: &str, remembered: &mut Option<Remembered>) -> Option<WindowInfo> {
        let wanted: Vec<u16> = title.encode_utf16().collect();

        // Quick path: the window found last time, if it is still there with
        // the same title.
        if let Some((remembered_title, handle)) = remembered.as_ref()
            && *remembered_title == wanted
        {
            let hwnd = *handle as HWND;
            // SAFETY: `IsWindow` accepts any handle value and says whether it is valid.
            if unsafe { IsWindow(hwnd) } != 0
                && title_of(hwnd) == wanted
                && let Some(info) = info_of(hwnd)
            {
                return Some(info);
            }
        }

        // Slow path: look at every top-level window.
        let mut search = Search { wanted: wanted.clone(), found: None };
        // SAFETY: `visit` only uses the pointer while `EnumWindows` runs, and
        // `search` outlives that call.
        unsafe {
            EnumWindows(Some(visit), &mut search as *mut Search as LPARAM);
        }
        match search.found {
            Some((handle, info)) => {
                *remembered = Some((wanted, handle));
                Some(info)
            }
            None => {
                *remembered = None;
                None
            }
        }
    }

    /// The handle of the window with this title.
    pub fn handle_of(title: &str) -> Option<isize> {
        let mut remembered = None;
        find_by_title(title, &mut remembered)?;
        remembered.map(|(_, handle)| handle as isize)
    }

    /// Called by Windows for every top-level window. Returning 0 stops the
    /// enumeration, 1 goes on.
    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> i32 {
        // SAFETY: `lparam` is the pointer `find_by_title` passed in.
        let search = unsafe { &mut *(lparam as *mut Search) };
        if title_of(hwnd) != search.wanted {
            return 1;
        }
        match info_of(hwnd) {
            Some(info) => {
                search.found = Some((hwnd as usize, info));
                0
            }
            None => 1,
        }
    }

    fn info_of(hwnd: HWND) -> Option<WindowInfo> {
        let rect = visible_rect(hwnd)?;
        // SAFETY: a plain query about a window handle.
        let minimized = unsafe { IsIconic(hwnd) } != 0;
        Some(WindowInfo { rect, minimized })
    }

    fn title_of(hwnd: HWND) -> Vec<u16> {
        // SAFETY: plain queries about a window handle.
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if length <= 0 {
            return Vec::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        buffer.truncate(copied.max(0) as usize);
        buffer
    }

    /// The visible frame of the window. Windows 10 and 11 add an invisible
    /// resize border around most windows, which `GetWindowRect` includes;
    /// the desktop window manager knows the real, visible bounds. If it
    /// cannot tell (it is off), `GetWindowRect` is the best there is.
    fn visible_rect(hwnd: HWND) -> Option<Rect> {
        // SAFETY: `RECT` is plain numbers; both calls fill it in.
        let mut r: RECT = unsafe { std::mem::zeroed() };
        let dwm_knows = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                // The library declares the constant as i32 and the parameter as u32.
                DWMWA_EXTENDED_FRAME_BOUNDS as u32,
                &mut r as *mut RECT as *mut c_void,
                size_of::<RECT>() as u32,
            )
        } == 0;
        if !dwm_knows && unsafe { GetWindowRect(hwnd, &mut r) } == 0 {
            return None;
        }
        Some(Rect { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top })
    }

    // -------------------------------------------------------------- watching

    struct Shared {
        wanted: Vec<u16>,
        /// The handle of the watched window once it has been seen (0 = not yet).
        window: AtomicUsize,
        changed: Mutex<bool>,
        wake: Condvar,
    }

    impl Shared {
        fn notify(&self) {
            *self.changed.lock().expect("the change flag") = true;
            self.wake.notify_all();
        }
    }

    thread_local! {
        /// The hook callback has no argument for our own data, but it always
        /// runs on the thread that installed the hook, so the data is found here.
        static CURRENT: RefCell<Option<Arc<Shared>>> = const { RefCell::new(None) };
    }

    pub struct Watcher {
        shared: Arc<Shared>,
        thread_id: u32,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Watcher {
        pub fn start(title: &str) -> Self {
            let shared = Arc::new(Shared {
                wanted: title.encode_utf16().collect(),
                window: AtomicUsize::new(0),
                changed: Mutex::new(false),
                wake: Condvar::new(),
            });
            let (ready_tx, ready_rx) = mpsc::channel::<u32>();
            let for_thread = shared.clone();
            let thread = std::thread::spawn(move || {
                CURRENT.with(|current| *current.borrow_mut() = Some(for_thread));
                // SAFETY: plain calls into the window system. The thread
                // first makes its message queue (the hook callbacks are
                // delivered through it), then installs the hook, then tells
                // `start` it may go on.
                unsafe {
                    let mut msg: MSG = std::mem::zeroed();
                    PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
                    let hook = SetWinEventHook(
                        EVENT_OBJECT_LOCATIONCHANGE,
                        EVENT_OBJECT_LOCATIONCHANGE,
                        null_mut(),
                        Some(on_event),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    );
                    let _ = ready_tx.send(GetCurrentThreadId());
                    if hook.is_null() {
                        return;
                    }
                    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    UnhookWinEvent(hook);
                }
            });
            let thread_id = ready_rx.recv().expect("the watcher thread starts");
            Watcher { shared, thread_id, thread: Some(thread) }
        }

        pub fn wait_for_change(&self, timeout: Duration) -> bool {
            let changed = self.shared.changed.lock().expect("the change flag");
            let (mut changed, _) = self
                .shared
                .wake
                .wait_timeout_while(changed, timeout, |changed| !*changed)
                .expect("the change flag");
            // True if a change woke us; the flag is cleared either way.
            std::mem::take(&mut *changed)
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            // SAFETY: asks the watcher thread to leave its message loop.
            unsafe {
                PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Windows calls this when any window anywhere changes position or size.
    unsafe extern "system" fn on_event(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        id_object: i32,
        id_child: i32,
        _event_thread: u32,
        _event_time: u32,
    ) {
        // Only the window itself, not its caret, cursor or controls.
        if id_object != OBJID_WINDOW || id_child != 0 || hwnd.is_null() {
            return;
        }
        CURRENT.with(|current| {
            let current = current.borrow();
            let Some(shared) = current.as_ref() else { return };
            let handle = hwnd as usize;
            if shared.window.load(Ordering::Relaxed) == handle {
                shared.notify();
            } else if title_of(hwnd) == shared.wanted {
                // First sighting of the watched window, or a new one with the same title.
                shared.window.store(handle, Ordering::Relaxed);
                shared.notify();
            }
        });
    }
}

/// The behaviour every locator must have, written once. Each system supplies
/// a `TestWindowHost` that opens real windows for it.
#[cfg(test)]
// Only Windows has a host that runs it so far; elsewhere it waits for one (see the ignored test below).
#[cfg_attr(not(windows), allow(dead_code))]
mod contract {
    use super::*;
    use std::time::{Duration, Instant};

    /// A real window opened for a test. Dropping it closes it.
    pub trait TestWindow {
        fn move_to(&mut self, rect: Rect);
        fn minimize(&mut self);
        fn restore(&mut self);
        fn close(&mut self);
    }

    pub trait TestWindowHost {
        fn open(&self, title: &str, rect: Rect) -> Box<dyn TestWindow>;
        /// A window with a title bar and frame, like an ordinary program's.
        fn open_framed(&self, title: &str, rect: Rect) -> Box<dyn TestWindow>;
    }

    const AT: Rect = Rect { x: 120, y: 80, width: 500, height: 700 };
    const ELSEWHERE: Rect = Rect { x: 300, y: 200, width: 400, height: 600 };

    pub fn unique(name: &str) -> String {
        format!("scrcpy-agent-test-{}-{name}", std::process::id())
    }

    /// The window system applies moves and closes a moment later, so look
    /// again until `done` is satisfied, for at most three seconds, and return
    /// what was seen last.
    pub fn eventually(
        locator: &dyn WindowLocator,
        title: &str,
        done: impl Fn(&Option<WindowInfo>) -> bool,
    ) -> Option<WindowInfo> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let found = locator.find_by_title(title);
            if done(&found) || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until the window system has stopped changing what it reports
    /// (the same answer for 300 ms), for at most three seconds, and returns
    /// it. Minimising is animated: asking for a restore while that still
    /// runs can leave the window half way, so a test waits for it to settle.
    fn settled(locator: &dyn WindowLocator, title: &str) -> Option<WindowInfo> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut last = locator.find_by_title(title);
        let mut unchanged_since = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let now = locator.find_by_title(title);
            if now != last {
                last = now;
                unchanged_since = Instant::now();
            }
            if unchanged_since.elapsed() >= Duration::from_millis(300) || Instant::now() >= deadline {
                return last;
            }
        }
    }

    /// How a watcher is made for a title.
    pub type MakeWatcher<'a> = &'a dyn Fn(&str) -> Box<dyn WindowWatcher>;

    /// Reads and throws away what is already waiting to be reported. It gives
    /// up after two seconds, so a watcher that never stops reporting makes the
    /// test fail instead of hanging it.
    fn drain(watcher: &dyn WindowWatcher) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && watcher.wait_for_change(Duration::from_millis(20)) {}
    }

    /// A move has to be reported at once. The window is moved 60 times, with
    /// a short pause between, like the frames of a drag; each move must wake
    /// the watcher, the typical delay must be a few frames at most, and no
    /// delay may be anywhere near the 100 ms of looking ten times a second.
    pub fn is_told_about_every_move_within_a_few_milliseconds(
        make_watcher: MakeWatcher,
        host: &dyn TestWindowHost,
    ) {
        let title = unique("watch-moves");
        let mut window = host.open(&title, AT);
        let watcher = make_watcher(&title);
        drain(watcher.as_ref());

        let mut delays = Vec::new();
        for step in 0..60 {
            let rect = Rect { x: 100 + step * 7, y: 80 + step * 3, ..AT };
            let started = Instant::now();
            window.move_to(rect);
            let told = watcher.wait_for_change(Duration::from_secs(1));
            assert!(told, "the watcher was not told about move {step}");
            delays.push(started.elapsed());
            std::thread::sleep(Duration::from_millis(15));
            drain(watcher.as_ref());
        }
        delays.sort();
        let (median, worst) = (delays[delays.len() / 2], *delays.last().expect("60 delays"));
        eprintln!("60 moves: median delay {median:?}, worst {worst:?}");
        assert!(median <= Duration::from_millis(30), "typical delay too long: {median:?}");
        assert!(worst <= Duration::from_millis(150), "worst delay too long: {worst:?}");
    }

    /// Only the watched window counts. Without this, a watcher that always
    /// says "something changed" would pass the test above.
    pub fn is_not_woken_by_other_windows(make_watcher: MakeWatcher, host: &dyn TestWindowHost) {
        let (watched, other) = (unique("watched"), unique("bystander"));
        let mut own = host.open(&watched, AT);
        let mut bystander = host.open(&other, ELSEWHERE);
        let watcher = make_watcher(&watched);
        drain(watcher.as_ref());

        // It does hear about its own window...
        own.move_to(ELSEWHERE);
        assert!(watcher.wait_for_change(Duration::from_secs(1)), "not told about its own window");
        drain(watcher.as_ref());

        // ...and not about the window next to it.
        for step in 0..10 {
            bystander.move_to(Rect { x: 200 + step * 5, ..ELSEWHERE });
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !watcher.wait_for_change(Duration::from_millis(300)),
            "woken by a window it does not watch"
        );
    }

    pub fn reports_the_position_and_size(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let title = unique("position");
        let _window = host.open(&title, AT);
        assert_eq!(locator.find_by_title(&title), Some(WindowInfo { rect: AT, minimized: false }));
    }

    pub fn follows_a_move_and_resize(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let title = unique("move");
        let mut window = host.open(&title, AT);
        window.move_to(ELSEWHERE);
        let seen = eventually(locator, &title, |f| f.is_some_and(|i| i.rect == ELSEWHERE));
        assert_eq!(seen, Some(WindowInfo { rect: ELSEWHERE, minimized: false }));
    }

    pub fn reports_minimising_and_restoring(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let title = unique("minimise");
        let mut window = host.open(&title, AT);
        window.minimize();
        let seen = settled(locator, &title);
        assert!(seen.is_some_and(|i| i.minimized), "not reported as minimised: {seen:?}");
        window.restore();
        // Windows takes about 80 ms to bring the window back. For that time
        // it already says "not minimised" while the position is still the
        // minimised one (-32000, -32000), so wait for the finished state.
        let restored = Some(WindowInfo { rect: AT, minimized: false });
        let seen = eventually(locator, &title, |f| *f == restored);
        assert_eq!(seen, restored);
    }

    pub fn finds_nothing_once_the_window_is_closed(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let title = unique("close");
        let mut window = host.open(&title, AT);
        assert!(locator.find_by_title(&title).is_some(), "the window was not found while open");
        window.close();
        assert_eq!(eventually(locator, &title, |f| f.is_none()), None);
    }

    pub fn finds_nothing_for_an_unknown_title(locator: &dyn WindowLocator, _host: &dyn TestWindowHost) {
        assert_eq!(locator.find_by_title(&unique("nobody-has-this-title")), None);
    }

    pub fn needs_the_whole_title_in_the_same_case(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let title = unique("exact");
        let _window = host.open(&format!("{title} extra"), AT);
        // The title is only the beginning of the real one.
        assert_eq!(locator.find_by_title(&title), None);
        // Same letters in a different case.
        assert_eq!(locator.find_by_title(&format!("{} EXTRA", title.to_uppercase())), None);
        // The real title is found.
        assert!(locator.find_by_title(&format!("{title} extra")).is_some());
    }

    pub fn tells_several_windows_apart(locator: &dyn WindowLocator, host: &dyn TestWindowHost) {
        let (first, second) = (unique("first"), unique("second"));
        let _a = host.open(&first, AT);
        let _b = host.open(&second, ELSEWHERE);
        assert_eq!(locator.find_by_title(&first).map(|i| i.rect), Some(AT));
        assert_eq!(locator.find_by_title(&second).map(|i| i.rect), Some(ELSEWHERE));
    }
}

/// Opens real windows on Windows by running the `test_window` example.
#[cfg(all(test, windows))]
mod windows_host {
    use super::Rect;
    use super::contract::{TestWindow, TestWindowHost};
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::Once;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };

    pub struct Host;

    struct Window {
        child: Child,
        stdin: ChildStdin,
    }

    fn helper() -> PathBuf {
        let exe = std::env::current_exe().expect("test executable path");
        let target_dir = exe.parent().and_then(|p| p.parent()).expect("target directory");
        let path = target_dir.join("examples").join("test_window.exe");
        assert!(
            path.exists(),
            "{} has not been built. Run the full `cargo test`: running a single test target \
             does not build the example programs.",
            path.display()
        );
        path
    }

    impl TestWindowHost for Host {
        fn open(&self, title: &str, rect: Rect) -> Box<dyn TestWindow> {
            start(title, rect, false)
        }

        fn open_framed(&self, title: &str, rect: Rect) -> Box<dyn TestWindow> {
            start(title, rect, true)
        }
    }

    /// Starts the `test_window` program. `framed` gives the window a title
    /// bar and a resize frame, and with them the invisible border.
    fn start(title: &str, rect: Rect, framed: bool) -> Box<dyn TestWindow> {
        // Coordinates must be physical pixels on both sides.
        static DPI: Once = Once::new();
        DPI.call_once(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        });
        let mut command = Command::new(helper());
        command
            .arg(title)
            .args([rect.x, rect.y, rect.width, rect.height].map(|n| n.to_string()));
        if framed {
            command.arg("framed");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("start test_window");
        let mut first_line = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut first_line)
            .expect("read from test_window");
        assert_eq!(first_line.trim(), "ready", "test_window did not start");
        let stdin = child.stdin.take().expect("stdin");
        Box::new(Window { child, stdin })
    }

    impl Window {
        fn send(&mut self, line: &str) {
            writeln!(self.stdin, "{line}").expect("write to test_window");
            self.stdin.flush().expect("flush");
        }
    }

    impl TestWindow for Window {
        fn move_to(&mut self, r: Rect) {
            self.send(&format!("move {} {} {} {}", r.x, r.y, r.width, r.height));
        }
        fn minimize(&mut self) {
            self.send("minimize");
        }
        fn restore(&mut self) {
            self.send("restore");
        }
        fn close(&mut self) {
            self.send("close");
            let _ = self.child.wait();
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    mod windows {
        use super::super::contract::{self as c, TestWindowHost};
        use crate::core::window::native::{hide, place_visible, set_owner};
        use super::super::windows_host::Host;
        use super::super::{Rect, WindowLocator, WindowsLocator, window_handle};

        // Windows 10 and 11 put an invisible border (a few pixels on the left,
        // right and bottom) around an ordinary program window. These tests use
        // a window with a title bar and frame, which has that border.
        const OUTER: Rect = Rect { x: 120, y: 80, width: 500, height: 700 };
        const WANTED: Rect = Rect { x: 500, y: 300, width: 400, height: 600 };

        #[test]
        fn a_framed_window_is_reported_by_its_visible_part_not_its_invisible_border() {
            let title = c::unique("framed-bounds");
            let _window = Host.open_framed(&title, OUTER);
            let seen = WindowsLocator::new().find_by_title(&title).expect("the window").rect;
            // The window was made with OUTER as its outer rectangle, so how far
            // the reported rectangle lies inside it is the invisible border.
            for (side, inset) in [
                ("left", seen.x - OUTER.x),
                ("right", OUTER.right() - seen.right()),
                ("bottom", OUTER.bottom() - seen.bottom()),
            ] {
                assert!((1..=16).contains(&inset), "{side} inset is {inset} px; reported {seen:?}");
            }
            // The top edge is the title bar, which is visible.
            assert_eq!(seen.y, OUTER.y);
        }

        #[test]
        fn placing_a_framed_window_puts_its_visible_part_where_it_was_asked() {
            let title = c::unique("place-framed");
            let _window = Host.open_framed(&title, OUTER);
            let handle = window_handle(&title).expect("the window handle");
            place_visible(handle, WANTED);
            let locator = WindowsLocator::new();
            let seen = c::eventually(&locator, &title, |f| f.is_some_and(|i| i.rect == WANTED));
            assert_eq!(seen.map(|i| i.rect), Some(WANTED));
        }

        #[test]
        fn placing_a_window_without_an_invisible_border_is_exact_too() {
            let title = c::unique("place-borderless");
            let _window = Host.open(&title, OUTER);
            let handle = window_handle(&title).expect("the window handle");
            place_visible(handle, WANTED);
            let locator = WindowsLocator::new();
            let seen = c::eventually(&locator, &title, |f| f.is_some_and(|i| i.rect == WANTED));
            assert_eq!(seen.map(|i| i.rect), Some(WANTED));
        }

        fn is_visible(window_handle: isize) -> bool {
            use windows_sys::Win32::Foundation::HWND;
            use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
            // SAFETY: a plain query about a window handle.
            unsafe { IsWindowVisible(window_handle as HWND) != 0 }
        }

        /// The scrcpy window gets minimised and the toolbar has to go away; it
        /// did not, because the window library took the toolbar for hidden
        /// already (it had been shown by placing it) and ignored `hide`.
        #[test]
        fn a_window_shown_by_placing_it_can_be_hidden_and_shown_again() {
            let title = c::unique("hide");
            let _window = Host.open(&title, OUTER);
            let handle = window_handle(&title).expect("the window handle");
            place_visible(handle, WANTED);
            assert!(is_visible(handle), "not visible after being placed");
            hide(handle);
            assert!(!is_visible(handle), "still visible after hide");
            place_visible(handle, WANTED);
            assert!(is_visible(handle), "not visible after being placed again");
        }

        /// Sends the window behind all the ordinary windows, without activating
        /// anything. (Bringing a window of another program to the front from
        /// outside is something Windows does not allow; sending it to the back
        /// it does, so the tests arrange the order that way.)
        fn lower(window_handle: isize) {
            use windows_sys::Win32::Foundation::HWND;
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
            };
            // SAFETY: `SetWindowPos` is given a window handle; a window that is gone just fails.
            unsafe {
                SetWindowPos(window_handle as HWND, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }

        /// The given windows from the front to the back, as Windows stacks them.
        fn front_to_back(handles: &[isize]) -> Vec<isize> {
            use windows_sys::Win32::Foundation::{HWND, LPARAM};
            use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
            unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> i32 {
                // SAFETY: `lparam` is the pointer to the list passed below.
                unsafe { (*(lparam as *mut Vec<isize>)).push(hwnd as isize) };
                1
            }
            let mut all: Vec<isize> = Vec::new();
            // SAFETY: the list outlives the call, which is where the pointer is used.
            unsafe {
                EnumWindows(Some(collect), &mut all as *mut Vec<isize> as LPARAM);
            }
            all.retain(|h| handles.contains(h));
            all
        }

        /// Three windows: an owner (the scrcpy window), a window that belongs to
        /// it (the toolbar) and the window of another program. They start as
        /// other-in-front (the last one made is on top), toolbar, owner.
        fn owner_tool_other(name: &str) -> (Vec<Box<dyn c::TestWindow>>, isize, isize, isize) {
            let titles = [c::unique(&format!("{name}-owner")), c::unique(&format!("{name}-tool")), c::unique(&format!("{name}-other"))];
            let windows: Vec<_> = titles.iter().map(|t| Host.open(t, OUTER)).collect();
            let handle = |title: &String| window_handle(title).expect("the window handle");
            let (owner, tool, other) = (handle(&titles[0]), handle(&titles[1]), handle(&titles[2]));
            (windows, owner, tool, other)
        }

        /// This is what was reported: the toolbar stayed in front of the other
        /// program's window after the scrcpy window had gone behind it. A
        /// window that has an owner goes down together with it.
        #[test]
        fn a_window_owned_by_another_goes_behind_other_windows_with_it() {
            let (_windows, owner, tool, other) = owner_tool_other("own-behind");
            set_owner(tool, owner);

            // The scrcpy window and its toolbar in front of the other window.
            lower(other);
            assert_eq!(front_to_back(&[tool, owner, other]), [tool, owner, other]);

            // Another window comes in front of the scrcpy window: the toolbar
            // goes behind it too instead of staying in front.
            lower(owner);
            assert_eq!(front_to_back(&[tool, owner, other]), [other, tool, owner]);
        }

        fn watcher(title: &str) -> Box<dyn super::super::WindowWatcher> {
            Box::new(super::super::WindowsWatcher::new(title))
        }

        #[test]
        fn is_told_about_every_move_within_a_few_milliseconds() {
            c::is_told_about_every_move_within_a_few_milliseconds(&watcher, &Host);
        }
        #[test]
        fn is_not_woken_by_other_windows() {
            c::is_not_woken_by_other_windows(&watcher, &Host);
        }

        #[test]
        fn reports_the_position_and_size_of_a_window() {
            c::reports_the_position_and_size(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn follows_a_move_and_resize() {
            c::follows_a_move_and_resize(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn reports_minimising_and_restoring() {
            c::reports_minimising_and_restoring(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn finds_nothing_once_the_window_is_closed() {
            c::finds_nothing_once_the_window_is_closed(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn finds_nothing_for_an_unknown_title() {
            c::finds_nothing_for_an_unknown_title(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn needs_the_whole_title_in_the_same_case() {
            c::needs_the_whole_title_in_the_same_case(&WindowsLocator::new(), &Host);
        }
        #[test]
        fn tells_several_windows_apart() {
            c::tells_several_windows_apart(&WindowsLocator::new(), &Host);
        }
    }

    /// A reminder that shows up in the test summary on systems without a
    /// locator yet: the same contract has to be run against theirs.
    #[cfg(not(windows))]
    #[test]
    #[ignore = "no locator for this system yet; run the shared contract against it"]
    fn the_locator_of_this_system_meets_the_contract() {}
}
