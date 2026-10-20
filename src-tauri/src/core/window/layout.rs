//! Where the floating toolbar goes relative to the scrcpy window, and whether
//! it may be attached to that window at all on the current system.
//!
//! Everything here is pure logic with no operating system calls, so it is the
//! same on every platform and fully testable.

/// A rectangle in screen pixels. `x`/`y` is the top-left corner; they can be
/// negative when a monitor sits left of or above the primary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }
}

/// Where to put a toolbar of size `toolbar` next to the window `target`.
///
/// * The toolbar sits on `side` of the window, `gap` pixels away, with its
///   top edge (left/right) or left edge (top/bottom) level with the window's.
/// * It must stay on the screen that holds the window's centre (the first
///   screen if the centre is on none). Whether the *side* fits is judged on
///   the axis it extends along only: left/right need the room horizontally,
///   top/bottom vertically.
/// * If the requested side does not fit, the opposite side is used.
/// * If neither fits, the requested side is kept and the toolbar is pushed
///   back onto the screen, so it overlaps the window instead of going missing.
/// * On the other axis the toolbar is always pushed back onto the screen.
pub fn dock_rect(target: Rect, toolbar: Size, side: Side, gap: i32, screens: &[Rect]) -> Rect {
    let wanted = place(target, toolbar, side, gap);
    let opposite = place(target, toolbar, side.opposite(), gap);
    let Some(screen) = screen_holding(target, screens) else {
        // No screen information: nothing to keep the toolbar inside of.
        return wanted;
    };
    let chosen = if fits_along_axis(wanted, side, screen) {
        wanted
    } else if fits_along_axis(opposite, side.opposite(), screen) {
        opposite
    } else {
        wanted
    };
    keep_on_screen(chosen, screen)
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    /// Whether the two rectangles share any area.
    pub fn overlaps(&self, other: &Rect) -> bool {
        self.x < other.right() && other.x < self.right() && self.y < other.bottom() && other.y < self.bottom()
    }
}

/// The toolbar on `side` of `target`, before any screen limits are applied.
fn place(target: Rect, toolbar: Size, side: Side, gap: i32) -> Rect {
    let (x, y) = match side {
        Side::Right => (target.right() + gap, target.y),
        Side::Left => (target.x - gap - toolbar.width, target.y),
        Side::Top => (target.x, target.y - gap - toolbar.height),
        Side::Bottom => (target.x, target.bottom() + gap),
    };
    Rect { x, y, width: toolbar.width, height: toolbar.height }
}

/// The screen that holds the centre of `target`, else the first screen.
fn screen_holding(target: Rect, screens: &[Rect]) -> Option<Rect> {
    let centre_x = target.x + target.width / 2;
    let centre_y = target.y + target.height / 2;
    screens
        .iter()
        .copied()
        .find(|s| centre_x >= s.x && centre_x < s.right() && centre_y >= s.y && centre_y < s.bottom())
        .or_else(|| screens.first().copied())
}

/// Whether the toolbar is inside the screen along the axis it extends along.
fn fits_along_axis(toolbar: Rect, side: Side, screen: Rect) -> bool {
    match side {
        Side::Left | Side::Right => toolbar.x >= screen.x && toolbar.right() <= screen.right(),
        Side::Top | Side::Bottom => toolbar.y >= screen.y && toolbar.bottom() <= screen.bottom(),
    }
}

/// Pushes the toolbar back inside the screen. A toolbar larger than the
/// screen is lined up with the screen's top-left corner.
fn keep_on_screen(toolbar: Rect, screen: Rect) -> Rect {
    let x = toolbar.x.min(screen.right() - toolbar.width).max(screen.x);
    let y = toolbar.y.min(screen.bottom() - toolbar.height).max(screen.y);
    Rect { x, y, ..toolbar }
}

// ------------------------------------------------------------------ platform

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    /// macOS, Android: no attached toolbar.
    Other,
}

impl Os {
    /// The system this program was built for.
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
}

/// The kind of Linux desktop session that is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionType {
    X11,
    Wayland,
}

/// The user's choice in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSetting {
    Auto,
    X11,
    Wayland,
}

/// The display system the windows are actually created on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Windows (and other systems with a single window system).
    Native,
    X11,
    Wayland,
}

/// The display system that will actually be used.
///
/// * Outside Linux there is only one, whatever the setting says.
/// * On Linux, `Auto` follows the running session. Choosing `X11` on a
///   Wayland session runs the windows through XWayland.
/// * `Wayland` cannot be honoured in an X11 session; X11 is used there.
pub fn effective_backend(os: Os, setting: BackendSetting, session: SessionType) -> Backend {
    match os {
        Os::Windows | Os::Other => Backend::Native,
        Os::Linux => match (setting, session) {
            (BackendSetting::X11, _) | (_, SessionType::X11) => Backend::X11,
            (BackendSetting::Auto | BackendSetting::Wayland, SessionType::Wayland) => Backend::Wayland,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Attached to the scrcpy window, on this side of it.
    Docked(Side),
    /// A separate window the user drags wherever they like.
    Free,
}

/// Why a requested placement could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Downgrade {
    /// Wayland has no global window coordinates, so a window cannot be
    /// attached to another one. X11 can.
    WaylandCannotDock,
    PlatformUnsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub placement: Placement,
    pub downgrade: Option<Downgrade>,
}

/// The placement that will really be used. Docking is available on Windows
/// and on Linux under X11. Elsewhere a docked request becomes a free window,
/// together with the reason, so the settings can explain it to the user.
pub fn resolve_placement(requested: Placement, os: Os, backend: Backend) -> Resolved {
    let free = |downgrade| Resolved { placement: Placement::Free, downgrade };
    match requested {
        Placement::Free => free(None),
        Placement::Docked(_) => match (os, backend) {
            (Os::Windows, _) | (Os::Linux, Backend::X11) => Resolved { placement: requested, downgrade: None },
            (Os::Linux, Backend::Wayland) => free(Some(Downgrade::WaylandCannotDock)),
            (Os::Linux, Backend::Native) | (Os::Other, _) => free(Some(Downgrade::PlatformUnsupported)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn r(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { x, y, width, height }
    }
    const fn sz(width: i32, height: i32) -> Size {
        Size { width, height }
    }
    const SCREEN: Rect = r(0, 0, 1920, 1080);
    const VERTICAL_BAR: Size = sz(56, 400);
    const HORIZONTAL_BAR: Size = sz(400, 56);

    // dock_rect: the four sides ------------------------------------------------

    #[test]
    fn docks_to_the_right_of_the_window_level_with_its_top() {
        let target = r(100, 50, 540, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &[SCREEN]), r(648, 50, 56, 400));
    }

    #[test]
    fn docks_to_the_left_of_the_window() {
        let target = r(300, 50, 540, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Left, 8, &[SCREEN]), r(236, 50, 56, 400));
    }

    #[test]
    fn docks_above_and_below_the_window_level_with_its_left_edge() {
        let target = r(100, 200, 540, 800);
        assert_eq!(dock_rect(target, HORIZONTAL_BAR, Side::Top, 8, &[SCREEN]), r(100, 136, 400, 56));
        assert_eq!(dock_rect(target, HORIZONTAL_BAR, Side::Bottom, 8, &[SCREEN]), r(100, 1008, 400, 56));
    }

    // dock_rect: not enough room -----------------------------------------------

    #[test]
    fn uses_the_opposite_side_when_the_requested_one_runs_off_the_screen() {
        // The window is at the right edge of the screen.
        let target = r(1300, 50, 600, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &[SCREEN]), r(1236, 50, 56, 400));
    }

    #[test]
    fn overlaps_the_window_rather_than_leaving_the_screen_when_neither_side_fits() {
        let target = r(0, 0, 1920, 1080);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &[SCREEN]), r(1864, 0, 56, 400));
    }

    #[test]
    fn keeps_the_toolbar_on_the_screen_along_the_other_axis() {
        // The window hangs off the bottom of the screen; the toolbar must not.
        let target = r(100, 900, 540, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &[SCREEN]), r(648, 680, 56, 400));
    }

    // dock_rect: several monitors ----------------------------------------------

    #[test]
    fn does_not_count_a_neighbouring_monitor_as_room_for_the_toolbar() {
        // Docking right would straddle the border between the two monitors.
        let screens = [r(0, 0, 1920, 1080), r(1920, 0, 1920, 1080)];
        let target = r(1300, 50, 600, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &screens), r(1236, 50, 56, 400));
    }

    #[test]
    fn works_with_a_monitor_at_negative_coordinates() {
        let screens = [r(-1920, 0, 1920, 1080), r(0, 0, 1920, 1080)];
        let target = r(-1500, 100, 500, 900);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &screens), r(-992, 100, 56, 400));
    }

    #[test]
    fn a_window_outside_every_monitor_is_handled_against_the_first_one() {
        let target = r(5000, 100, 500, 500);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &[SCREEN]), r(1864, 100, 56, 400));
    }

    #[test]
    fn a_window_on_the_second_monitor_is_judged_against_that_monitor() {
        // The window sits at the right edge of the second monitor, so the
        // toolbar does not fit on its right and goes to its left.
        let screens = [r(0, 0, 1920, 1080), r(1920, 0, 1920, 1080)];
        let target = r(3200, 50, 600, 1000);
        assert_eq!(dock_rect(target, VERTICAL_BAR, Side::Right, 8, &screens), r(3136, 50, 56, 400));
    }

    // effective_backend --------------------------------------------------------

    #[test]
    fn outside_linux_the_backend_setting_is_irrelevant() {
        for setting in [BackendSetting::Auto, BackendSetting::X11, BackendSetting::Wayland] {
            assert_eq!(effective_backend(Os::Windows, setting, SessionType::X11), Backend::Native);
        }
    }

    #[test]
    fn auto_follows_the_running_linux_session() {
        assert_eq!(effective_backend(Os::Linux, BackendSetting::Auto, SessionType::X11), Backend::X11);
        assert_eq!(
            effective_backend(Os::Linux, BackendSetting::Auto, SessionType::Wayland),
            Backend::Wayland
        );
    }

    #[test]
    fn choosing_x11_on_a_wayland_session_runs_through_xwayland() {
        assert_eq!(effective_backend(Os::Linux, BackendSetting::X11, SessionType::Wayland), Backend::X11);
    }

    #[test]
    fn wayland_cannot_be_used_in_an_x11_session() {
        assert_eq!(effective_backend(Os::Linux, BackendSetting::Wayland, SessionType::X11), Backend::X11);
        assert_eq!(
            effective_backend(Os::Linux, BackendSetting::Wayland, SessionType::Wayland),
            Backend::Wayland
        );
    }

    // resolve_placement --------------------------------------------------------

    fn resolved(placement: Placement, downgrade: Option<Downgrade>) -> Resolved {
        Resolved { placement, downgrade }
    }

    #[test]
    fn a_free_window_is_always_allowed() {
        for (os, backend) in [
            (Os::Windows, Backend::Native),
            (Os::Linux, Backend::X11),
            (Os::Linux, Backend::Wayland),
            (Os::Other, Backend::Native),
        ] {
            assert_eq!(resolve_placement(Placement::Free, os, backend), resolved(Placement::Free, None));
        }
    }

    #[test]
    fn docking_is_allowed_on_windows_and_on_linux_x11() {
        let docked = Placement::Docked(Side::Right);
        assert_eq!(resolve_placement(docked, Os::Windows, Backend::Native), resolved(docked, None));
        assert_eq!(resolve_placement(docked, Os::Linux, Backend::X11), resolved(docked, None));
    }

    #[test]
    fn on_wayland_docking_becomes_a_free_window_with_the_reason() {
        assert_eq!(
            resolve_placement(Placement::Docked(Side::Left), Os::Linux, Backend::Wayland),
            resolved(Placement::Free, Some(Downgrade::WaylandCannotDock))
        );
    }

    #[test]
    fn on_other_systems_docking_becomes_a_free_window_with_the_reason() {
        assert_eq!(
            resolve_placement(Placement::Docked(Side::Top), Os::Other, Backend::Native),
            resolved(Placement::Free, Some(Downgrade::PlatformUnsupported))
        );
    }
}
