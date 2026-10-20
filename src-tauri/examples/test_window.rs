//! A real window for the window-locator tests (never shipped).
//!
//! `test_window <title> <x> <y> <width> <height> [framed]` opens a borderless
//! window (or one with a title bar and frame) at that place and prints `ready`. The window cannot be seen, clicked or
//! focused (see below), so running the tests does not disturb anyone working
//! on the same computer. It then reads commands from stdin, one
//! per line: `move <x> <y> <width> <height>`, `minimize`, `restore`, `close`.
//! The window also closes when stdin is closed, so a test that panics does
//! not leave windows behind.
//!
//! It is DPI aware, so its coordinates are physical pixels, like those of the
//! application.

#[cfg(windows)]
mod imp {
    use std::io::{BufRead, Write};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_DESTROY => {
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    pub fn run() {
        let all: Vec<String> = std::env::args().skip(1).collect();
        // A last argument `framed` gives the window a title bar and a resize
        // frame, like a normal program window; Windows 10 and 11 then add an
        // invisible border around it, which is what the tests need to see.
        let framed = all.last().is_some_and(|arg| arg == "framed");
        let args = if framed { &all[..all.len() - 1] } else { &all[..] };
        let [title, x, y, width, height] = args else {
            eprintln!("usage: test_window <title> <x> <y> <width> <height> [framed]");
            std::process::exit(2);
        };
        let number = |text: &String| text.parse::<i32>().expect("a whole number");

        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let instance = GetModuleHandleW(null());
            let class_name = wide("ScrcpyAgentTestWindow");
            let mut class: WNDCLASSEXW = std::mem::zeroed();
            class.cbSize = size_of::<WNDCLASSEXW>() as u32;
            class.lpfnWndProc = Some(window_proc);
            class.hInstance = instance;
            class.lpszClassName = class_name.as_ptr();
            assert!(RegisterClassExW(&class) != 0, "RegisterClassExW failed");

            let title = wide(title);
            // The window is real to the system but invisible to the person at
            // the computer: fully transparent, clicks go through it, it never
            // takes the focus and it has no taskbar button.
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                class_name.as_ptr(),
                title.as_ptr(),
                if framed {
                    WS_OVERLAPPEDWINDOW | WS_VISIBLE
                } else {
                    WS_POPUP | WS_VISIBLE | WS_MINIMIZEBOX
                },
                number(x),
                number(y),
                number(width),
                number(height),
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null(), "CreateWindowExW failed");
            assert!(SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA) != 0, "could not make the window transparent");

            // Commands arrive on another thread; the window functions below
            // are safe to call from there, the window's own thread runs the
            // message loop.
            let handle = hwnd as usize;
            std::thread::spawn(move || {
                let hwnd = handle as HWND;
                for line in std::io::stdin().lock().lines() {
                    let Ok(line) = line else { break };
                    let words: Vec<&str> = line.split_whitespace().collect();
                    match words.as_slice() {
                        ["move", x, y, w, h] => {
                            MoveWindow(
                                hwnd,
                                x.parse().expect("x"),
                                y.parse().expect("y"),
                                w.parse().expect("width"),
                                h.parse().expect("height"),
                                1,
                            );
                        }
                        // The "no activate" forms: the plain ones would take the
                        // focus away from whoever is working on this computer.
                        ["minimize"] => {
                            ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                        }
                        // Windows brings a restored window to the front, and
                        // there is no way to restore one without that (every
                        // way of restoring was tried). So the focus is handed
                        // straight back to the window that had it.
                        ["restore"] => {
                            let previous = GetForegroundWindow();
                            ShowWindow(hwnd, SW_RESTORE);
                            if GetForegroundWindow() == hwnd && !previous.is_null() {
                                SetForegroundWindow(previous);
                            }
                        }
                        ["close"] => {
                            PostMessageW(hwnd, WM_CLOSE, 0, 0);
                        }
                        _ => {}
                    }
                }
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            });

            println!("ready");
            std::io::stdout().flush().expect("flush");

            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn main() {
    #[cfg(windows)]
    imp::run();
    #[cfg(not(windows))]
    {
        eprintln!("test_window only runs on Windows");
        std::process::exit(2);
    }
}
