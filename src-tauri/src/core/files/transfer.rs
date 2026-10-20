//! Sending files to a device: where they go, what they are called there and how
//! a path is given to the device's shell.

use std::path::Path;

/// Gives text to the device's shell as one word.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// A folder on the device that files are sent to: an absolute path without a
/// zero byte, a line break or any other control character, of at most 4096
/// bytes, and with no `..` part. A trailing `/` is dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDir(String);

impl RemoteDir {
    pub fn parse(text: &str) -> Option<Self> {
        if !text.starts_with('/') || text.len() > MAX_DIR || text.chars().any(char::is_control) {
            return None;
        }
        let mut path = String::new();
        for part in text.split('/').filter(|p| !p.is_empty()) {
            if part == "." || part == ".." {
                return None;
            }
            path.push('/');
            path.push_str(part);
        }
        Some(RemoteDir(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path of a file of this folder.
    pub fn join(&self, name: &str) -> String {
        format!("{}/{name}", self.0)
    }
}

/// The longest folder path (`PATH_MAX` of Linux).
const MAX_DIR: usize = 4096;

/// The longest name of a file (`NAME_MAX` of the file systems of Android).
const MAX_NAME: usize = 255;

/// The name a local file has on the device: its own, if it is a name (valid text,
/// no control characters, not empty, at most 255 bytes).
pub fn file_name_of(local: &Path) -> Option<String> {
    let name = local.file_name()?.to_str()?;
    (!name.is_empty() && name.len() <= MAX_NAME && !name.chars().any(char::is_control)).then(|| name.to_string())
}

/// `name` if no file of the folder has it, else the first of `name (1)`,
/// `name (2)`, ... that is free; the number goes before the last extension.
pub fn unique_name(existing: &[String], name: &str) -> String {
    let taken = |candidate: &str| existing.iter().any(|e| e == candidate);
    if !taken(name) {
        return name.to_string();
    }
    // A name that only starts with a dot has no extension.
    let (stem, extension) = match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    };
    (1u32..)
        .map(|n| format!("{stem} ({n}){extension}"))
        .find(|candidate| !taken(candidate))
        .expect("a number that is free")
}

/// The names `ls -1` printed: one per line.
pub fn parse_listing(output: &str) -> Vec<String> {
    output.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_for_the_shell_is_in_single_quotes_and_a_quote_in_it_is_closed_and_reopened() {
        assert_eq!(shell_quote("/sdcard/Download"), "'/sdcard/Download'");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("$(rm -rf x);`y`"), "'$(rm -rf x);`y`'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("''"), r"''\'''\'''");
    }

    #[test]
    fn a_folder_is_an_absolute_path_that_cannot_climb_out_or_carry_control_characters() {
        let ok = |t: &str| RemoteDir::parse(t).map(|d| d.as_str().to_string());
        assert_eq!(ok("/sdcard/Download/scrcpy-agent"), Some("/sdcard/Download/scrcpy-agent".into()));
        assert_eq!(ok("/sdcard/Download/scrcpy-agent/"), Some("/sdcard/Download/scrcpy-agent".into()));
        assert_eq!(ok("/sdcard/My Files"), Some("/sdcard/My Files".into()));
        assert_eq!(ok("/"), Some("".into()), "the root is the empty folder path, so a file is '/name'");
        for bad in ["", "sdcard/x", "./x", "/sdcard/../data", "/..", "/a/./b/../c", "/sdcard/a\nb", "/sdcard/a\0b", "/sdcard/a\tb"] {
            assert_eq!(ok(bad), None, "{bad:?}");
        }
        // A part that only looks like climbing is a name.
        assert_eq!(ok("/sdcard/..hidden"), Some("/sdcard/..hidden".into()));
        assert_eq!(ok(&format!("/{}", "a".repeat(4096))), None);
        assert_eq!(ok("//sdcard//Download"), Some("/sdcard/Download".into()), "empty parts are dropped");
    }

    #[test]
    fn a_file_of_a_folder_has_its_path() {
        let dir = RemoteDir::parse("/sdcard/Download/x/").unwrap();
        assert_eq!(dir.join("a.txt"), "/sdcard/Download/x/a.txt");
        assert_eq!(RemoteDir::parse("/sdcard/x").unwrap().join("a"), "/sdcard/x/a");
        assert_eq!(RemoteDir::parse("/").unwrap().join("a.txt"), "/a.txt");
    }

    #[test]
    fn the_name_on_the_device_is_the_local_name_if_it_is_a_name() {
        assert_eq!(file_name_of(Path::new("/somewhere/report.pdf")).as_deref(), Some("report.pdf"));
        assert_eq!(file_name_of(Path::new("/tmp/Bärbericht final.docx")).as_deref(), Some("Bärbericht final.docx"));
        assert_eq!(file_name_of(Path::new("/tmp/.hidden")).as_deref(), Some(".hidden"));
        assert_eq!(file_name_of(Path::new("")), None);
        assert_eq!(file_name_of(Path::new("/")), None);
        assert_eq!(file_name_of(Path::new("/tmp/a\nb.txt")), None);
        assert_eq!(file_name_of(Path::new("/tmp/a\u{7}b.txt")), None);
        let long = format!("/tmp/{}", "x".repeat(256));
        assert_eq!(file_name_of(Path::new(&long)), None);
        let longest = format!("/tmp/{}", "x".repeat(255));
        assert!(file_name_of(Path::new(&longest)).is_some());
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_name_that_is_taken_gets_a_number_before_its_last_extension() {
        let taken = names(&["a.txt", "a (1).txt", "b.tar.gz", ".bashrc", "noext"]);
        assert_eq!(unique_name(&taken, "free.txt"), "free.txt");
        assert_eq!(unique_name(&taken, "a.txt"), "a (2).txt");
        assert_eq!(unique_name(&taken, "b.tar.gz"), "b.tar (1).gz");
        assert_eq!(unique_name(&taken, ".bashrc"), ".bashrc (1)");
        assert_eq!(unique_name(&taken, "noext"), "noext (1)");
        assert_eq!(unique_name(&[], "a.txt"), "a.txt");
        // Names differing in case are different names on the device.
        assert_eq!(unique_name(&taken, "A.txt"), "A.txt");
    }

    #[test]
    fn the_listing_is_one_name_per_line_without_line_ends_or_empty_lines() {
        assert_eq!(parse_listing("a.txt\nb c.pdf\r\n\nreport.docx\n"), names(&["a.txt", "b c.pdf", "report.docx"]));
        assert_eq!(parse_listing(""), Vec::<String>::new());
        assert_eq!(parse_listing("\n\n"), Vec::<String>::new());
    }
}
