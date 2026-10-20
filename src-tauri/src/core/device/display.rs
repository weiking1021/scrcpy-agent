//! The size and orientation of a device's screen as it is held now, read from
//! what Android reports. An app opened in a display of its own gets the same
//! orientation as the screen the user is using.

/// A size in pixels; both sides are at least 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Landscape,
    Portrait,
}

impl ScreenSize {
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(ScreenSize { width, height })
    }

    /// A square screen counts as portrait: it is not wider than it is tall.
    pub fn orientation(self) -> Orientation {
        if self.width > self.height {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        }
    }
}

/// The adb shell command whose output `current_size` reads.
pub const DISPLAY_DUMP_COMMAND: &str = "dumpsys display";

/// The size of the built-in display (id 0) in the orientation it has now: the
/// `real <w> x <h>` field of the `mOverrideDisplayInfo` line, which follows the
/// rotation (`mBaseDisplayInfo` always has the natural one). `None` if there is
/// no such line or field, or it is not exactly in that form; the caller then
/// says so instead of guessing.
pub fn current_size(dump: &str) -> Option<ScreenSize> {
    dump.lines().find_map(|line| {
        let info = line.trim_start().strip_prefix("mOverrideDisplayInfo=DisplayInfo{")?;
        let fields: Vec<&str> = info.split(", ").collect();
        // The display's own id comes as `displayId 0"` after its quoted name.
        if !fields.iter().any(|f| f.trim_end_matches('"') == "displayId 0") {
            return None;
        }
        fields.iter().find_map(|f| {
            let (w, h) = f.strip_prefix("real ")?.split_once(" x ")?;
            ScreenSize::new(w.parse().ok()?, h.parse().ok()?)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines in the form `dumpsys display` prints for a device held sideways. The
    // sizes and every other value are invented; only the form of the lines was
    // checked against a real device, once.
    const SIDEWAYS: &str = "\
    mBaseDisplayInfo=DisplayInfo{\"Display\", displayId 0\", displayGroupId 0, FLAG_TRUSTED, real 1200 x 1920, largest app 1200 x 1920, smallest app 1200 x 1920, appVsyncOff 2000000
    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", displayGroupId 0, FLAG_TRUSTED, real 1920 x 1200, largest app 1920 x 1800, smallest app 1200 x 1100, appVsyncOff 2000000
";

    #[test]
    fn the_size_is_the_one_the_screen_has_now_not_the_natural_one() {
        let size = current_size(SIDEWAYS).expect("a size");
        assert_eq!(size, ScreenSize { width: 1920, height: 1200 });
        assert_eq!(size.orientation(), Orientation::Landscape);
    }

    #[test]
    fn an_upright_screen_is_portrait() {
        let upright = SIDEWAYS.replace("real 1920 x 1200", "real 1200 x 1920");
        let size = current_size(&upright).expect("a size");
        assert_eq!(size, ScreenSize { width: 1200, height: 1920 });
        assert_eq!(size.orientation(), Orientation::Portrait);
        assert_eq!(ScreenSize::new(1000, 1000).unwrap().orientation(), Orientation::Portrait);
    }

    #[test]
    fn only_the_built_in_display_counts() {
        let other = SIDEWAYS.replace("displayId 0\"", "displayId 7\"");
        assert_eq!(current_size(&other), None);
    }

    #[test]
    fn what_is_not_exactly_a_size_is_not_taken_for_one() {
        for text in [
            "",
            "    mBaseDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 1200 x 1920, x",
            "    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 0 x 1200, x",
            "    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 12a x 1200, x",
            "    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", real 1920 x, x",
            "    mOverrideDisplayInfo=DisplayInfo{\"Display\", displayId 0\", x",
        ] {
            assert_eq!(current_size(text), None, "{text:?}");
        }
    }
}
