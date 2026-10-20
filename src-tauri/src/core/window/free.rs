//! How a window that has no frame and a fixed size (the toolbar) is put in its
//! place, as a list of steps: the same on every system, and the order of the steps
//! is what can be tested without a window system. `toolbar_window` carries the
//! steps out on a real window.
//!
//! Why the steps differ: on Linux (GTK) a window that cannot be resized ignores the
//! size it is given afterwards, and stays as large as it was made (800x600), so it
//! has to be allowed to resize while it is sized, and fixed again after. Windows
//! does not need this, and places the toolbar with its own call (`native_window`).

use super::layout::{Os, Rect};

/// One call on a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    SetResizable(bool),
    /// In physical pixels.
    SetSize { width: u32, height: u32 },
    /// In physical pixels.
    SetPosition { x: i32, y: i32 },
    Show,
}

/// Whether the window is made resizable. The toolbar is not resizable by the user;
/// on Linux it starts out resizable only so that it can be sized (see the module).
pub fn created_resizable(os: Os) -> bool {
    os == Os::Linux
}

/// The calls that put a window at `rect` and show it, in order.
pub fn steps_to_place(os: Os, rect: Rect) -> Vec<Step> {
    // An empty or negative size is one pixel: it never becomes a huge unsigned number.
    let size = Step::SetSize { width: rect.width.max(1) as u32, height: rect.height.max(1) as u32 };
    let position = Step::SetPosition { x: rect.x, y: rect.y };
    if os == Os::Linux {
        vec![Step::SetResizable(true), size, position, Step::Show, Step::SetResizable(false)]
    } else {
        vec![size, position, Step::Show]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = Rect { x: 80, y: -20, width: 56, height: 184 };
    const SIZE: Step = Step::SetSize { width: 56, height: 184 };
    const POSITION: Step = Step::SetPosition { x: 80, y: -20 };

    #[test]
    fn on_linux_the_window_may_resize_while_it_is_sized_and_is_fixed_after_it_is_shown() {
        assert_eq!(
            steps_to_place(Os::Linux, RECT),
            [Step::SetResizable(true), SIZE, POSITION, Step::Show, Step::SetResizable(false)]
        );
    }

    #[test]
    fn elsewhere_the_window_is_sized_placed_and_shown_and_its_resizing_is_left_alone() {
        for os in [Os::Windows, Os::Other] {
            assert_eq!(steps_to_place(os, RECT), [SIZE, POSITION, Step::Show], "{os:?}");
        }
    }

    #[test]
    fn a_size_that_is_empty_or_negative_becomes_one_pixel_not_a_huge_number() {
        let rect = Rect { x: 0, y: 0, width: 0, height: -5 };
        for os in [Os::Linux, Os::Windows, Os::Other] {
            let steps = steps_to_place(os, rect);
            assert!(steps.contains(&Step::SetSize { width: 1, height: 1 }), "{os:?}: {steps:?}");
        }
    }

    #[test]
    fn only_on_linux_the_window_is_made_resizable_to_begin_with() {
        assert!(created_resizable(Os::Linux));
        assert!(!created_resizable(Os::Windows));
        assert!(!created_resizable(Os::Other));
    }
}
