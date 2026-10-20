//! The toolbar as a Tauri window, and the task that keeps it next to the
//! scrcpy window. The decisions (where to put it, when to hide it) are made by
//! `core::window::toolbar::Follower`; this file only turns them into window calls.
//!
//! The follower is not run on a timer. It runs when the window system reports
//! that the scrcpy window moved (`WindowWatcher`), which keeps the toolbar a
//! few milliseconds behind the window; a slow timer is only the safety net for
//! what no event announces, like the window appearing.

use crate::core::{Core, OpenedWindow};
use crate::core::window::free::{Step, created_resizable, steps_to_place};
use crate::core::window::layout::{Os, Rect, Size};
use crate::core::model::ToolbarSettings;
use crate::core::window::toolbar::{DOCK_GAP, Follower, Progress, ToolbarHost, bar_size};
use crate::core::window::locator::{WindowLocator, WindowWatcher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl, WebviewWindowBuilder};

/// The longest the follower goes without looking, event or not.
const SAFETY_LOOK: Duration = Duration::from_millis(250);

/// How long the list of screens and their scale is trusted before it is
/// asked for again. Asking goes through the main thread, which is too slow to
/// do on every move.
const MONITORS_FRESH_FOR: Duration = Duration::from_secs(2);

/// Where a toolbar that is not attached starts, from the top-left corner of
/// the primary screen.
const FREE_START: i32 = 80;

static NEXT_TOOLBAR: AtomicU64 = AtomicU64::new(1);

/// Finds the scrcpy window. Only Windows has a locator so far; elsewhere the
/// toolbar is a free window the user drags.
fn system_locator() -> Option<Box<dyn WindowLocator + Send>> {
    #[cfg(windows)]
    {
        Some(Box::new(crate::core::window::locator::WindowsLocator::new()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Hears about the scrcpy window moving, where the system can tell.
fn system_watcher(title: &str) -> Option<Box<dyn WindowWatcher>> {
    #[cfg(windows)]
    {
        Some(Box::new(crate::core::window::locator::WindowsWatcher::new(title)))
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        None
    }
}

struct TauriToolbar<R: Runtime> {
    app: AppHandle<R>,
    label: String,
    device: String,
    /// The title of the scrcpy window this toolbar belongs to.
    #[cfg(windows)]
    scrcpy_title: String,
    /// The scrcpy window that has been made the owner of the toolbar window.
    #[cfg(windows)]
    owner: Option<isize>,
    always_on_top: bool,
    attached: bool,
}

impl<R: Runtime> TauriToolbar<R> {
    fn window(&self) -> Option<tauri::WebviewWindow<R>> {
        use tauri::Manager;
        self.app.get_webview_window(&self.label)
    }

    fn create(&self) -> Option<tauri::WebviewWindow<R>> {
        // The page learns which device it belongs to, and whether it can be
        // dragged, from its address. The `url` crate does the escaping.
        let mut address = tauri::Url::parse("http://localhost/").ok()?;
        address
            .query_pairs_mut()
            .append_pair("device", &self.device)
            .append_pair("mode", if self.attached { "docked" } else { "free" });
        let page = format!("toolbar.html?{}", address.query()?);
        WebviewWindowBuilder::new(&self.app, &self.label, WebviewUrl::App(page.into()))
            .title("scrcpy-agent toolbar")
            .decorations(false)
            .always_on_top(self.always_on_top)
            .skip_taskbar(true)
            .resizable(created_resizable(Os::current()))
            .focused(false)
            .visible(false)
            .build()
            .ok()
    }
}

impl<R: Runtime> ToolbarHost for TauriToolbar<R> {
    fn show_at(&mut self, rect: Rect) {
        let Some(window) = self.window().or_else(|| self.create()) else { return };
        #[cfg(windows)]
        if let Ok(handle) = window.hwnd() {
            let handle = handle.0 as isize;
            // Unless it is asked to float above everything, the toolbar is
            // owned by the scrcpy window: it stays in front of that window,
            // goes behind other programs' windows together with it, and goes
            // away when it is minimised or closed.
            if !self.always_on_top {
                let scrcpy = crate::core::window::locator::window_handle(&self.scrcpy_title);
                if scrcpy != self.owner {
                    if let Some(scrcpy) = scrcpy {
                        crate::core::window::native::set_owner(handle, scrcpy);
                    }
                    self.owner = scrcpy;
                }
            }
            // One call moves, resizes and shows the window, and makes its
            // visible part (not its invisible border) land on `rect`. Asking
            // Tauri for each of those separately costs three trips to its
            // main thread, which shows as lag while the scrcpy window is dragged.
            crate::core::window::native::place_visible(handle, rect);
            return;
        }
        for step in steps_to_place(Os::current(), rect) {
            let _ = match step {
                Step::SetResizable(resizable) => window.set_resizable(resizable),
                Step::SetSize { width, height } => window.set_size(PhysicalSize::new(width, height)),
                Step::SetPosition { x, y } => window.set_position(PhysicalPosition::new(x, y)),
                Step::Show => window.show(),
            };
        }
    }

    fn hide(&mut self) {
        let Some(window) = self.window() else { return };
        // The window was shown by `place_visible`, which the window library
        // does not know about; its own `hide` would take the window for
        // hidden already and do nothing. So the same side that shows hides.
        #[cfg(windows)]
        if let Ok(handle) = window.hwnd() {
            crate::core::window::native::hide(handle.0 as isize);
            return;
        }
        let _ = window.hide();
    }

    fn close(&mut self) {
        if let Some(window) = self.window() {
            let _ = window.destroy();
        }
    }
}

/// The screens and the scale of the primary one, asked for at most every
/// couple of seconds. (A setup with screens of different scales gets the
/// primary one's size everywhere.)
#[derive(Default)]
struct Monitors {
    fetched: Option<(Instant, Vec<Rect>, f64)>,
}

impl Monitors {
    fn get<R: Runtime>(&mut self, app: &AppHandle<R>) -> (Vec<Rect>, f64) {
        let stale = self.fetched.as_ref().is_none_or(|(at, ..)| at.elapsed() > MONITORS_FRESH_FOR);
        if stale {
            let screens = app
                .available_monitors()
                .unwrap_or_default()
                .iter()
                .map(|m| Rect {
                    x: m.position().x,
                    y: m.position().y,
                    width: m.size().width as i32,
                    height: m.size().height as i32,
                })
                .collect();
            let scale = app.primary_monitor().ok().flatten().map_or(1.0, |m| m.scale_factor());
            self.fetched = Some((Instant::now(), screens, scale));
        }
        let (_, screens, scale) = self.fetched.as_ref().expect("just fetched");
        (screens.clone(), *scale)
    }
}

fn free_start<R: Runtime>(app: &AppHandle<R>, size: Size) -> Rect {
    let origin = app
        .primary_monitor()
        .ok()
        .flatten()
        .map_or((0, 0), |m| (m.position().x, m.position().y));
    Rect { x: origin.0 + FREE_START, y: origin.1 + FREE_START, width: size.width, height: size.height }
}

/// Shows the toolbar for one open scrcpy window and keeps it in place until
/// that window is gone.
pub async fn keep_toolbar<R: Runtime>(
    app: AppHandle<R>,
    core: Arc<Core>,
    device: String,
    title: String,
    settings: ToolbarSettings,
) {
    // Waiting for the window system blocks a thread, so the loop gets one of its own.
    let _ = tokio::task::spawn_blocking(move || follow(app, core, device, title, settings)).await;
}

/// The window of an app has a toolbar like the device's own screen has; its
/// buttons go by the key of the window.
pub fn keep_app_toolbar<R: Runtime>(app: AppHandle<R>, core: Arc<Core>, opened: OpenedWindow) {
    let toolbar = core.snapshot().settings.toolbar;
    if toolbar.enabled {
        tauri::async_runtime::spawn(keep_toolbar(app, core, opened.key, opened.title, toolbar));
    }
}

fn follow<R: Runtime>(
    app: AppHandle<R>,
    core: Arc<Core>,
    device: String,
    title: String,
    settings: ToolbarSettings,
) {
    let locator = system_locator();
    let watcher = system_watcher(&title);
    let attached = locator.is_some();
    let buttons = core.sessions().toolbar_buttons(Some(&device)).len();
    let mut host = TauriToolbar {
        app: app.clone(),
        label: format!("toolbar-{}", NEXT_TOOLBAR.fetch_add(1, Ordering::Relaxed)),
        device: device.clone(),
        #[cfg(windows)]
        scrcpy_title: title.clone(),
        #[cfg(windows)]
        owner: None,
        always_on_top: settings.always_on_top,
        attached,
    };
    let mut follower = Follower::new(&title, settings.side, DOCK_GAP);
    let mut monitors = Monitors::default();
    let mut placed_free = false;

    loop {
        if !core.sessions().has_session(&device) {
            host.close();
            return;
        }
        let (screens, scale) = monitors.get(&app);
        let size = bar_size(buttons, attached, scale);
        match &locator {
            Some(locator) => {
                if follower.tick(locator.as_ref(), &screens, size, &mut host) == Progress::Finished {
                    return;
                }
            }
            None if !placed_free => {
                host.show_at(free_start(&app, size));
                placed_free = true;
            }
            None => {}
        }
        match &watcher {
            Some(watcher) => {
                watcher.wait_for_change(SAFETY_LOOK);
            }
            None => std::thread::sleep(SAFETY_LOOK),
        }
    }
}
