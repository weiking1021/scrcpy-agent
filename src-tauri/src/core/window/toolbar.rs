//! Keeps the floating toolbar next to the scrcpy window.
//!
//! `Follower` looks at the scrcpy window once per tick and tells a
//! `ToolbarHost` what to do with the toolbar. It touches neither the window
//! system nor the toolbar itself, so it is tested with a fake window locator
//! and a fake host on every system.

use super::layout::{Rect, Side, Size, dock_rect};
use super::locator::WindowLocator;

/// Whatever puts the toolbar on screen: a Tauri window in the application, a
/// recording fake in the tests.
pub trait ToolbarHost {
    /// Shows the toolbar at this place (physical screen pixels), or moves it
    /// there.
    fn show_at(&mut self, rect: Rect);
    /// Hides the toolbar but keeps it, because it will come back.
    fn hide(&mut self);
    /// Removes the toolbar for good.
    fn close(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Running,
    /// The scrcpy window is gone and the toolbar was closed; nothing more
    /// will happen.
    Finished,
}

/// How far the toolbar stays from the scrcpy window, in physical pixels.
pub const DOCK_GAP: i32 = 8;

/// The size of the toolbar window in physical pixels.
///
/// The toolbar is a column of `buttons` buttons, 56 wide and 44 high (in
/// logical pixels, the size the screen scale `scale` then multiplies), with
/// 8 of padding in total. A toolbar that is not attached to the scrcpy window
/// also carries a 20 high grip to drag it by. Results are rounded to whole
/// pixels.
pub fn bar_size(buttons: usize, attached: bool, scale: f64) -> Size {
    let grip = if attached { 0.0 } else { GRIP_HEIGHT };
    let height = buttons as f64 * BUTTON_HEIGHT + BAR_PADDING + grip;
    Size {
        width: (BAR_WIDTH * scale).round() as i32,
        height: (height * scale).round() as i32,
    }
}

// Logical pixels; the page of the toolbar (toolbar.css) uses the same numbers.
const BAR_WIDTH: f64 = 56.0;
const BUTTON_HEIGHT: f64 = 44.0;
const BAR_PADDING: f64 = 8.0;
const GRIP_HEIGHT: f64 = 20.0;

/// What the host is showing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    Nothing,
    At(Rect),
    Hidden,
}

pub struct Follower {
    title: String,
    side: Side,
    gap: i32,
    /// The window has been found at least once.
    seen: bool,
    shown: Shown,
    finished: bool,
}

impl Follower {
    /// Follows the window with exactly this title, keeping the toolbar `gap`
    /// pixels away on `side`.
    pub fn new(title: &str, side: Side, gap: i32) -> Self {
        Follower {
            title: title.to_string(),
            side,
            gap,
            seen: false,
            shown: Shown::Nothing,
            finished: false,
        }
    }

    /// One look at the window, and the matching change to the toolbar.
    ///
    /// * Until the window has been seen once, nothing happens: scrcpy needs a
    ///   moment to open it.
    /// * When it is seen, the toolbar is shown at `dock_rect` next to it, and
    ///   moved whenever the window or the toolbar size changes. Nothing is
    ///   sent while nothing changed.
    /// * While the window is minimised the toolbar is hidden (once), and shown
    ///   again at the window when it is restored.
    /// * A position that overlaps no screen is ignored: Windows reports one
    ///   for a moment while it restores a window. (With no screen information
    ///   at all, every position counts as on a screen.)
    /// * When a window that was seen is gone, the toolbar is closed and the
    ///   follower is `Finished` for good.
    pub fn tick(
        &mut self,
        locator: &dyn WindowLocator,
        screens: &[Rect],
        toolbar: Size,
        host: &mut dyn ToolbarHost,
    ) -> Progress {
        if self.finished {
            return Progress::Finished;
        }
        let Some(window) = locator.find_by_title(&self.title) else {
            if !self.seen {
                return Progress::Running;
            }
            host.close();
            self.finished = true;
            return Progress::Finished;
        };
        self.seen = true;

        if window.minimized {
            if matches!(self.shown, Shown::At(_)) {
                host.hide();
                self.shown = Shown::Hidden;
            }
            return Progress::Running;
        }
        if !on_a_screen(window.rect, screens) {
            return Progress::Running;
        }
        let place = dock_rect(window.rect, toolbar, self.side, self.gap, screens);
        if self.shown != Shown::At(place) {
            host.show_at(place);
            self.shown = Shown::At(place);
        }
        Progress::Running
    }
}

/// Whether the rectangle overlaps any screen. Without screen information
/// every position counts as on a screen.
fn on_a_screen(rect: Rect, screens: &[Rect]) -> bool {
    screens.is_empty() || screens.iter().any(|screen| rect.overlaps(screen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::window::locator::WindowInfo;
    use std::cell::RefCell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Call {
        Show(Rect),
        Hide,
        Close,
    }

    #[derive(Default)]
    struct FakeHost {
        calls: Vec<Call>,
    }

    impl ToolbarHost for FakeHost {
        fn show_at(&mut self, rect: Rect) {
            self.calls.push(Call::Show(rect));
        }
        fn hide(&mut self) {
            self.calls.push(Call::Hide);
        }
        fn close(&mut self) {
            self.calls.push(Call::Close);
        }
    }

    /// A window that can be put somewhere, minimised, or taken away; it only
    /// answers to the title it was made for.
    struct FakeLocator {
        title: &'static str,
        window: RefCell<Option<WindowInfo>>,
    }

    impl FakeLocator {
        fn new() -> Self {
            FakeLocator { title: "Test Tablet", window: RefCell::new(None) }
        }
        fn put(&self, rect: Rect) {
            *self.window.borrow_mut() = Some(WindowInfo { rect, minimized: false });
        }
        fn minimise(&self) {
            let mut window = self.window.borrow_mut();
            assert!(window.is_some(), "there has to be a window to minimise");
            // Where Windows parks a minimised window.
            let parked = Rect { x: -32000, y: -32000, width: 160, height: 28 };
            *window = Some(WindowInfo { rect: parked, minimized: true });
        }
        fn remove(&self) {
            *self.window.borrow_mut() = None;
        }
    }

    impl WindowLocator for FakeLocator {
        fn find_by_title(&self, title: &str) -> Option<WindowInfo> {
            if title == self.title { *self.window.borrow() } else { None }
        }
    }

    const fn r(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect { x, y, width, height }
    }
    const SCREEN: Rect = r(0, 0, 1920, 1080);
    const BAR: Size = Size { width: 56, height: 400 };
    const WINDOW: Rect = r(100, 50, 540, 1000);
    /// Where the toolbar goes for `WINDOW`, on its right with a gap of 8.
    const BESIDE_WINDOW: Rect = r(648, 50, 56, 400);

    struct Rig {
        follower: Follower,
        locator: FakeLocator,
        host: FakeHost,
    }

    impl Rig {
        fn new() -> Self {
            Rig {
                follower: Follower::new("Test Tablet", Side::Right, 8),
                locator: FakeLocator::new(),
                host: FakeHost::default(),
            }
        }
        fn tick(&mut self) -> Progress {
            self.tick_with(BAR)
        }
        fn tick_with(&mut self, toolbar: Size) -> Progress {
            self.follower.tick(&self.locator, &[SCREEN], toolbar, &mut self.host)
        }
    }

    #[test]
    fn the_bar_is_56_wide_and_44_high_per_button_plus_8_of_padding() {
        assert_eq!(bar_size(10, true, 1.0), Size { width: 56, height: 448 });
        assert_eq!(bar_size(7, true, 1.0), Size { width: 56, height: 316 });
        assert_eq!(bar_size(0, true, 1.0), Size { width: 56, height: 8 });
    }

    #[test]
    fn a_bar_that_is_not_attached_has_room_for_a_grip() {
        assert_eq!(bar_size(10, false, 1.0), Size { width: 56, height: 468 });
    }

    #[test]
    fn the_bar_grows_with_the_screen_scale_and_is_rounded_to_whole_pixels() {
        assert_eq!(bar_size(10, true, 1.5), Size { width: 84, height: 672 });
        assert_eq!(bar_size(10, true, 2.0), Size { width: 112, height: 896 });
        // 56 x 1.1 = 61.6 and 448 x 1.1 = 492.8
        assert_eq!(bar_size(10, true, 1.1), Size { width: 62, height: 493 });
        // 56 x 1.25 = 70 and 468 x 1.25 = 585 (with the grip)
        assert_eq!(bar_size(10, false, 1.25), Size { width: 70, height: 585 });
        // 56 x 1.2 = 67.2 goes down to 67, not up to 68; 448 x 1.2 = 537.6 goes up to 538.
        assert_eq!(bar_size(10, true, 1.2), Size { width: 67, height: 538 });
    }

    #[test]
    fn waits_quietly_until_the_window_exists() {
        let mut rig = Rig::new();
        for _ in 0..3 {
            assert_eq!(rig.tick(), Progress::Running);
        }
        assert_eq!(rig.host.calls, []);
    }

    #[test]
    fn shows_the_toolbar_next_to_the_window_when_it_appears() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        assert_eq!(rig.tick(), Progress::Running);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW)]);
    }

    #[test]
    fn says_nothing_while_the_window_stays_where_it_is() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        for _ in 0..4 {
            assert_eq!(rig.tick(), Progress::Running);
        }
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW)]);
    }

    #[test]
    fn follows_the_window_when_it_moves() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        rig.tick();
        rig.locator.put(r(300, 200, 540, 1000));
        assert_eq!(rig.tick(), Progress::Running);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Show(r(848, 200, 56, 400))]);
    }

    #[test]
    fn hides_while_minimised_and_shows_again_in_the_same_place_after_the_restore() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        rig.tick();
        rig.locator.minimise();
        rig.tick();
        rig.tick();
        // Hidden once, not once per tick.
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Hide]);
        rig.locator.put(WINDOW);
        rig.tick();
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Hide, Call::Show(BESIDE_WINDOW)]);
    }

    #[test]
    fn ignores_a_position_that_is_on_no_screen() {
        let mut rig = Rig::new();
        let parked = r(-32000, -32000, 160, 28);
        // Not yet seen at a real place: nothing to show.
        rig.locator.put(parked);
        assert_eq!(rig.tick(), Progress::Running);
        assert_eq!(rig.host.calls, []);
        // Once shown, a moment at the parking place (Windows does this while
        // restoring) changes nothing...
        rig.locator.put(WINDOW);
        rig.tick();
        rig.locator.put(parked);
        rig.tick();
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW)]);
        // ...and the real position afterwards is followed.
        rig.locator.put(r(300, 200, 540, 1000));
        rig.tick();
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Show(r(848, 200, 56, 400))]);
    }

    #[test]
    fn closes_when_the_window_is_gone_and_stays_finished() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        rig.tick();
        rig.locator.remove();
        assert_eq!(rig.tick(), Progress::Finished);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Close]);
        // Later ticks do nothing, even if a window with that title returns.
        rig.locator.put(WINDOW);
        assert_eq!(rig.tick(), Progress::Finished);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Close]);
    }

    #[test]
    fn closes_even_when_the_window_vanishes_while_minimised() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        rig.tick();
        rig.locator.minimise();
        rig.tick();
        rig.locator.remove();
        assert_eq!(rig.tick(), Progress::Finished);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Hide, Call::Close]);
    }

    #[test]
    fn moves_the_toolbar_when_its_own_size_changes() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        rig.tick();
        assert_eq!(rig.tick_with(Size { width: 56, height: 500 }), Progress::Running);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW), Call::Show(r(648, 50, 56, 500))]);
    }

    #[test]
    fn keeps_to_the_side_it_was_given_and_flips_near_a_screen_edge() {
        let mut left = Rig::new();
        left.follower = Follower::new("Test Tablet", Side::Left, 8);
        left.locator.put(WINDOW);
        left.tick();
        assert_eq!(left.host.calls, [Call::Show(r(36, 50, 56, 400))]);

        let mut edge = Rig::new();
        edge.locator.put(r(1300, 50, 600, 1000));
        edge.tick();
        assert_eq!(edge.host.calls, [Call::Show(r(1236, 50, 56, 400))]);
    }

    #[test]
    fn only_looks_for_the_title_it_was_given() {
        let mut rig = Rig::new();
        rig.follower = Follower::new("Some Other Window", Side::Right, 8);
        rig.locator.put(WINDOW);
        assert_eq!(rig.tick(), Progress::Running);
        assert_eq!(rig.host.calls, []);
    }

    #[test]
    fn without_screen_information_the_toolbar_is_still_placed() {
        let mut rig = Rig::new();
        rig.locator.put(WINDOW);
        let progress = rig.follower.tick(&rig.locator, &[], BAR, &mut rig.host);
        assert_eq!(progress, Progress::Running);
        assert_eq!(rig.host.calls, [Call::Show(BESIDE_WINDOW)]);
    }
}
