//! Shortcuts that open an app of a device: the name of the file, and what it
//! runs. Writing the file is done by the system's own code (`shell_menu`); this
//! is what goes into it.

use crate::core::apps::PackageName;
use crate::core::window::layout::Os;

/// What a shortcut is made of on a system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutKind {
    /// A Windows shell link, `.lnk`.
    WindowsLink,
    /// A desktop entry of the Linux desktops, `.desktop`.
    DesktopEntry,
}

impl ShortcutKind {
    /// The kind of shortcut a system has; `None` where there are none.
    pub fn of(os: Os) -> Option<ShortcutKind> {
        match os {
            Os::Windows => Some(ShortcutKind::WindowsLink),
            Os::Linux => Some(ShortcutKind::DesktopEntry),
            Os::Other => None,
        }
    }

    /// The extension of the file, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ShortcutKind::WindowsLink => "lnk",
            ShortcutKind::DesktopEntry => "desktop",
        }
    }
}

/// The name that is shown for the shortcut (in the menu of the system, under its icon):
/// `<app> - <device>`; a line break or tab in a name is a space, and it is never empty.
pub fn display_name(app: &str, device: &str) -> String {
    let clean = |text: &str| text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string();
    let parts: Vec<String> = [clean(app), clean(device)].into_iter().filter(|p| !p.is_empty()).collect();
    if parts.is_empty() { "app".to_string() } else { parts.join(" - ") }
}

/// The name of the shortcut file for an app on a device: `<app> - <device>` and the
/// extension of the kind, with what Windows does not allow in a file name removed (on
/// every system, so the names are the same), a name of a reserved device (`CON`, `NUL`,
/// `COM1`...) made different, and at most 120 characters before the extension. Never empty.
pub fn file_name(app: &str, device: &str, kind: ShortcutKind) -> String {
    /// What Windows does not allow in a file name is `_`; spaces at the ends and dots at the end go.
    fn clean(text: &str) -> String {
        let replaced: String =
            text.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
        replaced.trim().trim_end_matches(['.', ' ']).to_string()
    }
    /// The names Windows keeps for devices, with or without an extension.
    fn reserved(stem: &str) -> bool {
        let first = stem.split('.').next().unwrap_or("").to_ascii_uppercase();
        matches!(first.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|p| first.strip_prefix(p).is_some_and(|n| matches!(n.len(), 1) && n != "0" && n.bytes().all(|b| b.is_ascii_digit())))
    }
    let parts: Vec<String> = [clean(app), clean(device)].into_iter().filter(|p| !p.is_empty()).collect();
    let mut stem = if parts.is_empty() { "app".to_string() } else { parts.join(" - ") };
    if reserved(&stem) {
        stem.push('_');
    }
    if stem.chars().count() > MAX_STEM {
        stem = stem.chars().take(MAX_STEM).collect::<String>().trim_end_matches(['.', ' ']).to_string();
    }
    format!("{stem}.{}", kind.extension())
}

/// Where a shortcut is put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutPlace {
    Desktop,
    StartMenu,
}

/// A shortcut this program made: whose app it is, where, and the file it is (the path it was
/// written to, whatever name the device had then).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShortcutRecord {
    pub package: String,
    pub place: ShortcutPlace,
    pub path: String,
}

/// The shortcuts made for one device (a kind of data of the device, `store::Shortcuts`). The menu
/// goes by these, not by the name a shortcut would get now: a device can be renamed, and a
/// shortcut that was made under the old name is still its shortcut.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ShortcutRecords {
    pub entries: Vec<ShortcutRecord>,
}

impl ShortcutRecords {
    pub fn find(&self, package: &str, place: ShortcutPlace) -> Option<&ShortcutRecord> {
        self.entries.iter().find(|r| r.package == package && r.place == place)
    }

    /// Records a shortcut; one of the same app in the same place is replaced. Returns what it replaced.
    pub fn set(&mut self, record: ShortcutRecord) -> Option<ShortcutRecord> {
        let old = self.remove(&record.package, record.place);
        self.entries.push(record);
        old
    }

    /// Takes the record of an app in a place away; the record that was.
    pub fn remove(&mut self, package: &str, place: ShortcutPlace) -> Option<ShortcutRecord> {
        let at = self.entries.iter().position(|r| r.package == package && r.place == place)?;
        Some(self.entries.remove(at))
    }
}

/// The most characters of a shortcut's name before its extension.
const MAX_STEM: usize = 120;

/// The arguments the shortcut gives the program: `--app`, the device, the package.
pub fn arguments(device_id: &str, package: &PackageName) -> Vec<String> {
    vec!["--app".into(), device_id.to_string(), package.as_str().to_string()]
}

/// The arguments as one line, the way a Windows shortcut has them: each is in
/// double quotes, and a double quote inside one is written `\"` (and the
/// backslashes before it doubled), so the program reads back the same arguments.
pub fn command_line(arguments: &[String]) -> String {
    let quote = |argument: &str| {
        let mut out = String::from("\"");
        let mut backslashes = 0;
        for c in argument.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.push_str(&"\\".repeat(backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                _ => {
                    out.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                    out.push(c);
                }
            }
        }
        // Before the closing quote they would escape it, so they are doubled.
        out.push_str(&"\\".repeat(backslashes * 2));
        out.push('"');
        out
    };
    arguments.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    use ShortcutKind::{DesktopEntry, WindowsLink};

    fn lnk(app: &str, device: &str) -> String {
        file_name(app, device, WindowsLink)
    }

    #[test]
    fn a_system_has_the_kind_of_shortcut_it_has_and_where_there_is_none_there_is_none() {
        assert_eq!(ShortcutKind::of(Os::Windows), Some(WindowsLink));
        assert_eq!(ShortcutKind::of(Os::Linux), Some(DesktopEntry));
        assert_eq!(ShortcutKind::of(Os::Other), None);
        assert_eq!((WindowsLink.extension(), DesktopEntry.extension()), ("lnk", "desktop"));
    }

    #[test]
    fn the_file_is_named_after_the_app_and_the_device() {
        assert_eq!(lnk("Notes", "Living room"), "Notes - Living room.lnk");
        // The same name on every system, with the extension of the kind.
        assert_eq!(file_name("Notes", "Living room", DesktopEntry), "Notes - Living room.desktop");
        assert_eq!(file_name("", "", DesktopEntry), "app.desktop");
        assert_eq!(file_name("a/b", "c:d", DesktopEntry), "a_b - c_d.desktop");
    }

    #[test]
    fn what_a_file_name_cannot_have_is_taken_out_and_it_is_never_empty() {
        assert_eq!(lnk("a<b>c:d\"e/f\\g|h?i*j", "dev"), "a_b_c_d_e_f_g_h_i_j - dev.lnk");
        assert_eq!(lnk("tab\there\nnew", "d"), "tab_here_new - d.lnk");
        // Spaces at the ends and dots at the end are not allowed by Windows.
        assert_eq!(lnk("  Notes. ", "dev. "), "Notes - dev.lnk");
        assert_eq!(lnk("", ""), "app.lnk");
        assert_eq!(lnk("   ", "..."), "app.lnk");
        assert_eq!(lnk("", "dev"), "dev.lnk");
        assert_eq!(lnk("Notes", ""), "Notes.lnk");
    }

    #[test]
    fn a_name_that_is_a_reserved_device_of_windows_is_made_different() {
        assert_eq!(lnk("CON", ""), "CON_.lnk");
        assert_eq!(lnk("nul", ""), "nul_.lnk");
        assert_eq!(lnk("Com1", ""), "Com1_.lnk");
        assert_eq!(lnk("LPT9", ""), "LPT9_.lnk");
        // Only the whole name is reserved; and "COM10" is an ordinary name.
        assert_eq!(lnk("CONSOLE", ""), "CONSOLE.lnk");
        assert_eq!(lnk("COM10", ""), "COM10.lnk");
    }

    #[test]
    fn a_long_name_is_cut_by_characters_not_bytes() {
        // A character of three bytes: a cut by bytes would keep fewer than 120 of them.
        let long = "€".repeat(300);
        let name = lnk(&long, "d");
        assert_eq!(name.chars().count(), 120 + ".lnk".len());
        assert!(name.ends_with(".lnk"));
        assert!(name.starts_with('€'));
        assert_eq!(file_name(&long, "d", DesktopEntry).chars().count(), 120 + ".desktop".len());
    }

    fn record(package: &str, place: ShortcutPlace, path: &str) -> ShortcutRecord {
        ShortcutRecord { package: package.into(), place, path: path.into() }
    }

    #[test]
    fn a_record_is_found_by_the_app_and_the_place_and_one_that_is_made_again_replaces_the_old() {
        use ShortcutPlace::{Desktop, StartMenu};
        let mut records = ShortcutRecords::default();
        assert_eq!(records.find("a.b", Desktop), None);
        assert_eq!(records.set(record("a.b", Desktop, "/d/one")), None);
        assert_eq!(records.set(record("a.b", StartMenu, "/m/one")), None);
        assert_eq!(records.set(record("c.d", Desktop, "/d/two")), None);
        assert_eq!(records.find("a.b", Desktop).unwrap().path, "/d/one");
        assert_eq!(records.find("a.b", StartMenu).unwrap().path, "/m/one");
        // The same app in the same place: the new one is the record, and the old one is told.
        assert_eq!(records.set(record("a.b", Desktop, "/d/new")), Some(record("a.b", Desktop, "/d/one")));
        assert_eq!(records.entries.len(), 3);
        assert_eq!(records.find("a.b", Desktop).unwrap().path, "/d/new");
        // Taking away: only that one, and once.
        assert_eq!(records.remove("a.b", Desktop), Some(record("a.b", Desktop, "/d/new")));
        assert_eq!(records.remove("a.b", Desktop), None);
        assert_eq!(records.entries.len(), 2);
        assert!(records.find("a.b", StartMenu).is_some() && records.find("c.d", Desktop).is_some());
    }

    #[test]
    fn the_records_are_kept_in_a_form_that_reads_back_and_a_file_without_them_is_empty() {
        let mut records = ShortcutRecords::default();
        records.set(record("a.b", ShortcutPlace::StartMenu, "/m/a b.desktop"));
        let text = serde_json::to_string(&records).unwrap();
        assert!(text.contains(r#""place":"start_menu""#), "{text}");
        assert_eq!(serde_json::from_str::<ShortcutRecords>(&text).unwrap(), records);
        assert_eq!(serde_json::from_str::<ShortcutRecords>("{}").unwrap(), ShortcutRecords::default());
    }

    #[test]
    fn the_name_shown_is_the_app_and_the_device_as_they_are_and_never_empty() {
        assert_eq!(display_name("Notes", "Living room"), "Notes - Living room");
        // Nothing is taken out of it: it is a name to read, not a file name.
        assert_eq!(display_name("a/b: c", "Den"), "a/b: c - Den");
        assert_eq!(display_name("  Notes ", " dev "), "Notes - dev");
        assert_eq!(display_name("tab\there\nnew", "d"), "tab here new - d");
        assert_eq!(display_name("Notes", ""), "Notes");
        assert_eq!(display_name("", "dev"), "dev");
        assert_eq!(display_name("  ", ""), "app");
    }

    #[test]
    fn the_program_is_given_the_app_flag_the_device_and_the_package() {
        let package = PackageName::parse("com.example.notes").unwrap();
        assert_eq!(arguments("TESTDEV0001", &package), ["--app", "TESTDEV0001", "com.example.notes"]);
    }

    #[test]
    fn the_arguments_are_one_line_with_each_in_quotes_and_a_quote_inside_written_out() {
        let line = |a: &[&str]| command_line(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(line(&["--app", "TESTDEV0001", "com.example.notes"]), r#""--app" "TESTDEV0001" "com.example.notes""#);
        assert_eq!(line(&["a b"]), r#""a b""#);
        assert_eq!(line(&[r#"say "hi""#]), r#""say \"hi\"""#);
        // Backslashes before a quote are doubled, and so are those at the end (before the closing quote).
        assert_eq!(line(&[r#"a\"b"#]), r#""a\\\"b""#);
        assert_eq!(line(&[r"C:\dir\"]), r#""C:\dir\\""#);
        assert_eq!(line(&[r"C:\dir\file"]), r#""C:\dir\file""#);
        assert_eq!(line(&[""]), r#""""#);
        assert_eq!(line(&[]), "");
    }
}
