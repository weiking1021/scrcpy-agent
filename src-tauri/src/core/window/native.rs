//! Placing one of our own windows on Windows.

use super::layout::Rect;

/// Puts the window so that its *visible* part is exactly `visible`.
///
/// Windows 10 and 11 add an invisible resize border (about 7 pixels on the
/// left, right and bottom) around most windows. `SetWindowPos` places the
/// outer rectangle including that border, so asking for `visible` as the
/// outer rectangle leaves the window that much too far in, and too narrow and
/// too short. The border is measured on the window itself and added back.
///
/// The window is shown, and not given the focus. Everything happens in one
/// call, so a window that follows another one stays in step with it.
#[cfg(windows)]
pub fn place_visible(window_handle: isize, visible: Rect) {
    use windows_sys::Win32::Foundation::HWND;

    let hwnd = window_handle as HWND;
    // A window that has never been shown may not report its border yet. It
    // is then placed with the border it reports (none), measured again once
    // it is on screen, and corrected if that turned out different.
    let mut used = Insets::default();
    for _ in 0..2 {
        let measured = insets_of(hwnd).unwrap_or_default();
        if measured == used && used != Insets::default() {
            break;
        }
        used = measured;
        set_outer_rect(hwnd, grow(visible, used));
        if insets_of(hwnd).unwrap_or_default() == used {
            break;
        }
    }
}

/// Makes `owner_handle` the owner of the window.
///
/// An owned window always stays above its owner, rises and sinks with it,
/// goes when the owner is minimised and is destroyed with it. It does not
/// float above other windows the way a topmost window does, which is what a
/// toolbar that belongs to one program window should do.
#[cfg(windows)]
pub fn set_owner(window_handle: isize, owner_handle: isize) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWLP_HWNDPARENT, SetWindowLongPtrW};
    // For a top-level window, GWLP_HWNDPARENT sets its owner (despite the name).
    // SAFETY: `SetWindowLongPtrW` is given window handles; if one of the
    // windows is gone the call just fails.
    unsafe {
        SetWindowLongPtrW(window_handle as HWND, GWLP_HWNDPARENT, owner_handle);
    }
}

/// Hides the window. A window that `place_visible` showed has to be hidden
/// with this and not with the window library's own `hide`: the library does
/// not know the window was shown behind its back, takes it for hidden
/// already, and then does nothing.
#[cfg(windows)]
pub fn hide(window_handle: isize) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
    // SAFETY: `ShowWindow` is given a window handle; if the window is already
    // gone the call just fails.
    unsafe {
        ShowWindow(window_handle as HWND, SW_HIDE);
    }
}

/// How far the invisible border reaches out on each side of the visible part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Insets {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `visible` with the invisible border added around it.
fn grow(visible: Rect, insets: Insets) -> Rect {
    Rect {
        x: visible.x - insets.left,
        y: visible.y - insets.top,
        width: visible.width + insets.left + insets.right,
        height: visible.height + insets.top + insets.bottom,
    }
}

#[cfg(windows)]
fn insets_of(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<Insets> {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

    // SAFETY: `RECT` is plain numbers; both calls fill it in.
    let mut outer: RECT = unsafe { std::mem::zeroed() };
    let mut visible: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut outer) } == 0 {
        return None;
    }
    let dwm_knows = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            // The library declares the constant as i32 and the parameter as u32.
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            &mut visible as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    } == 0;
    if !dwm_knows {
        return None;
    }
    Some(Insets {
        left: visible.left - outer.left,
        top: visible.top - outer.top,
        right: outer.right - visible.right,
        bottom: outer.bottom - visible.bottom,
    })
}

#[cfg(windows)]
fn set_outer_rect(hwnd: windows_sys::Win32::Foundation::HWND, rect: Rect) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
    };
    // SAFETY: `SetWindowPos` is given a window handle; if the window is
    // already gone the call just fails.
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
        );
    }
}
