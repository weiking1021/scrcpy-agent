//! The user's choices for the scrcpy window, as typed settings, and how they
//! become scrcpy's command line. The option names come from scrcpy's own
//! manual (`scrcpy.1`), not from memory.

/// A directory on the Android device, for example `/sdcard/Download/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevicePath(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidDevicePath;

impl DevicePath {
    /// Accepts an absolute path (it starts with `/`) without control
    /// characters such as a line break or NUL.
    pub fn parse(text: &str) -> Result<Self, InvalidDevicePath> {
        if text.strip_prefix('/').is_none() || text.chars().any(char::is_control) {
            return Err(InvalidDevicePath);
        }
        Ok(DevicePath(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The folder scrcpy pushes dropped files to when nothing else is set.
pub const DEFAULT_PUSH_TARGET: &str = "/sdcard/Download/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorSettings {
    pub audio: bool,
    pub borderless: bool,
    pub always_on_top: bool,
    pub fullscreen: bool,
    /// Mirror only; the device cannot be controlled from the window.
    pub view_only: bool,
    pub show_touches: bool,
    pub stay_awake: bool,
    pub turn_screen_off: bool,
    pub clipboard_autosync: bool,
    pub push_target: DevicePath,
    /// Where the window starts; scrcpy chooses when `None`.
    pub geometry: Option<WindowGeometry>,
    /// Arguments passed to scrcpy as they are, after all the others.
    pub extra_args: Vec<String>,
}

impl Default for MirrorSettings {
    fn default() -> Self {
        MirrorSettings {
            audio: true,
            borderless: false,
            always_on_top: false,
            fullscreen: false,
            view_only: false,
            show_touches: false,
            stay_awake: false,
            turn_screen_off: false,
            clipboard_autosync: true,
            push_target: DevicePath(DEFAULT_PUSH_TARGET.to_string()),
            geometry: None,
            extra_args: Vec::new(),
        }
    }
}

/// The device a window is opened for.
pub struct MirrorTarget<'a> {
    pub serial: &'a str,
    pub title: &'a str,
    /// Android API level, when known. Audio forwarding needs 30 (Android 11).
    pub sdk: Option<u32>,
}

/// The scrcpy command line for these settings.
///
/// Only what differs from scrcpy's own defaults is added, in a fixed order:
/// `--serial`, `--window-title`, window position and size, window style
/// (`--window-borderless`, `--always-on-top`, `--fullscreen`), device
/// options (`--no-control`, `--show-touches`, `--stay-awake`,
/// `--turn-screen-off`), `--no-audio`, `--no-clipboard-autosync`,
/// `--push-target`, and finally `extra_args`. `--no-audio` is also added for
/// a device older than Android 11, but never twice.
pub fn mirror_args(settings: &MirrorSettings, target: &MirrorTarget) -> Vec<String> {
    let mut args = vec![
        format!("--serial={}", target.serial),
        format!("--window-title={}", target.title),
    ];
    if let Some(g) = settings.geometry {
        args.push(format!("--window-x={}", g.x));
        args.push(format!("--window-y={}", g.y));
        args.push(format!("--window-width={}", g.width));
        args.push(format!("--window-height={}", g.height));
    }
    let flags = [
        (settings.borderless, "--window-borderless"),
        (settings.always_on_top, "--always-on-top"),
        (settings.fullscreen, "--fullscreen"),
        (settings.view_only, "--no-control"),
        (settings.show_touches, "--show-touches"),
        (settings.stay_awake, "--stay-awake"),
        (settings.turn_screen_off, "--turn-screen-off"),
        (!settings.audio || !can_forward_audio(target.sdk), "--no-audio"),
        (!settings.clipboard_autosync, "--no-clipboard-autosync"),
    ];
    args.extend(flags.into_iter().filter(|(on, _)| *on).map(|(_, flag)| flag.to_string()));
    if settings.push_target.as_str() != DEFAULT_PUSH_TARGET {
        args.push(format!("--push-target={}", settings.push_target.as_str()));
    }
    args.extend(settings.extra_args.iter().cloned());
    args
}

/// Audio forwarding needs Android 11. When the version is unknown scrcpy is
/// left to try.
fn can_forward_audio(sdk: Option<u32>) -> bool {
    sdk.is_none_or(|s| s >= super::session::MIN_AUDIO_SDK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> MirrorTarget<'static> {
        MirrorTarget { serial: "192.168.250.139:36600", title: "Test Tablet", sdk: Some(34) }
    }
    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }
    const BASE: [&str; 2] = ["--serial=192.168.250.139:36600", "--window-title=Test Tablet"];

    fn with(change: impl FnOnce(&mut MirrorSettings)) -> Vec<String> {
        let mut s = MirrorSettings::default();
        change(&mut s);
        mirror_args(&s, &target())
    }

    fn expected(extra: &[&str]) -> Vec<String> {
        let mut all: Vec<&str> = BASE.to_vec();
        all.extend_from_slice(extra);
        args(&all)
    }

    #[test]
    fn the_default_settings_add_nothing_to_scrcpys_own_defaults() {
        assert_eq!(mirror_args(&MirrorSettings::default(), &target()), expected(&[]));
    }

    #[test]
    fn each_window_option_becomes_its_scrcpy_option() {
        assert_eq!(with(|s| s.borderless = true), expected(&["--window-borderless"]));
        assert_eq!(with(|s| s.always_on_top = true), expected(&["--always-on-top"]));
        assert_eq!(with(|s| s.fullscreen = true), expected(&["--fullscreen"]));
    }

    #[test]
    fn each_device_option_becomes_its_scrcpy_option() {
        assert_eq!(with(|s| s.view_only = true), expected(&["--no-control"]));
        assert_eq!(with(|s| s.show_touches = true), expected(&["--show-touches"]));
        assert_eq!(with(|s| s.stay_awake = true), expected(&["--stay-awake"]));
        assert_eq!(with(|s| s.turn_screen_off = true), expected(&["--turn-screen-off"]));
    }

    #[test]
    fn switching_off_audio_or_clipboard_sync_adds_the_matching_option() {
        assert_eq!(with(|s| s.audio = false), expected(&["--no-audio"]));
        assert_eq!(with(|s| s.clipboard_autosync = false), expected(&["--no-clipboard-autosync"]));
    }

    #[test]
    fn a_device_older_than_android_11_gets_no_audio() {
        let s = MirrorSettings::default();
        let old = MirrorTarget { sdk: Some(29), ..target() };
        assert_eq!(mirror_args(&s, &old), expected(&["--no-audio"]));
        let exactly_11 = MirrorTarget { sdk: Some(30), ..target() };
        assert_eq!(mirror_args(&s, &exactly_11), expected(&[]));
        let unknown = MirrorTarget { sdk: None, ..target() };
        assert_eq!(mirror_args(&s, &unknown), expected(&[]));
    }

    #[test]
    fn no_audio_is_never_added_twice() {
        let s = MirrorSettings { audio: false, ..MirrorSettings::default() };
        let old = MirrorTarget { sdk: Some(29), ..target() };
        assert_eq!(mirror_args(&s, &old), expected(&["--no-audio"]));
    }

    #[test]
    fn the_window_position_and_size_are_passed_on() {
        let geometry = WindowGeometry { x: 100, y: -50, width: 540, height: 1000 };
        assert_eq!(
            with(|s| s.geometry = Some(geometry)),
            expected(&["--window-x=100", "--window-y=-50", "--window-width=540", "--window-height=1000"])
        );
    }

    #[test]
    fn a_push_target_other_than_the_default_is_passed_on() {
        let movies = DevicePath::parse("/sdcard/Movies/").unwrap();
        assert_eq!(with(|s| s.push_target = movies), expected(&["--push-target=/sdcard/Movies/"]));
        // The default is left to scrcpy.
        let default = DevicePath::parse(DEFAULT_PUSH_TARGET).unwrap();
        assert_eq!(with(|s| s.push_target = default), expected(&[]));
    }

    #[test]
    fn a_title_stays_one_argument_whatever_it_contains() {
        let odd = MirrorTarget { title: "--no-control  and spaces", ..target() };
        let got = mirror_args(&MirrorSettings::default(), &odd);
        assert_eq!(got, args(&["--serial=192.168.250.139:36600", "--window-title=--no-control  and spaces"]));
    }

    #[test]
    fn the_order_is_fixed_and_extra_arguments_come_last() {
        let all_on = MirrorSettings {
            audio: false,
            borderless: true,
            always_on_top: true,
            fullscreen: true,
            view_only: true,
            show_touches: true,
            stay_awake: true,
            turn_screen_off: true,
            clipboard_autosync: false,
            push_target: DevicePath::parse("/sdcard/Movies/").unwrap(),
            geometry: Some(WindowGeometry { x: 1, y: 2, width: 3, height: 4 }),
            extra_args: args(&["--max-fps=60", "--video-bit-rate=8M"]),
        };
        assert_eq!(
            mirror_args(&all_on, &target()),
            expected(&[
                "--window-x=1",
                "--window-y=2",
                "--window-width=3",
                "--window-height=4",
                "--window-borderless",
                "--always-on-top",
                "--fullscreen",
                "--no-control",
                "--show-touches",
                "--stay-awake",
                "--turn-screen-off",
                "--no-audio",
                "--no-clipboard-autosync",
                "--push-target=/sdcard/Movies/",
                "--max-fps=60",
                "--video-bit-rate=8M",
            ])
        );
    }

    #[test]
    fn device_paths_must_be_absolute_and_free_of_control_characters() {
        assert_eq!(DevicePath::parse("/sdcard/Movies/").unwrap().as_str(), "/sdcard/Movies/");
        assert_eq!(DevicePath::parse("/").unwrap().as_str(), "/");
        for bad in ["", "sdcard/Movies", "./x", "/sdcard/\nMovies", "/sdcard/Mov\0ies", "/a\tb"] {
            assert_eq!(DevicePath::parse(bad), Err(InvalidDevicePath), "{bad:?}");
        }
    }
}
