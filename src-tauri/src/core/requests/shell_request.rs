//! What the Windows Explorer menu asks of the application: it starts the program
//! with `--shell <action> <file>`, once for every file that was selected.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellAction {
    /// Send to the default device.
    SendDefault,
    /// Send to a device chosen from the list.
    SendChoose,
    /// Send to the default device and open there.
    OpenDefault,
    /// Send to a device chosen from the list and open there.
    OpenChoose,
}

impl ShellAction {
    /// The word that stands for it on the command line.
    pub fn word(self) -> &'static str {
        match self {
            ShellAction::SendDefault => "send-default",
            ShellAction::SendChoose => "send-choose",
            ShellAction::OpenDefault => "open-default",
            ShellAction::OpenChoose => "open-choose",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        [ShellAction::SendDefault, ShellAction::SendChoose, ShellAction::OpenDefault, ShellAction::OpenChoose]
            .into_iter()
            .find(|a| a.word() == word)
    }

    /// Whether the file is opened on the device after it is sent.
    pub fn opens(self) -> bool {
        matches!(self, ShellAction::OpenDefault | ShellAction::OpenChoose)
    }

    /// Whether the device is the default one (else it is chosen).
    pub fn uses_default(self) -> bool {
        matches!(self, ShellAction::SendDefault | ShellAction::OpenDefault)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRequest {
    pub action: ShellAction,
    pub file: PathBuf,
}

/// The requests in the arguments the program was started with (the first is the
/// program itself): `--shell`, an action and one or more files, a request for each
/// file. Anything else is not a request.
pub fn parse_args(args: &[String]) -> Option<Vec<ShellRequest>> {
    let [_program, flag, action, files @ ..] = args else { return None };
    if flag != "--shell" || files.is_empty() || files.iter().any(String::is_empty) {
        return None;
    }
    let action = ShellAction::parse(action)?;
    Some(files.iter().map(|file| ShellRequest { action, file: PathBuf::from(file) }).collect())
}

/// A request from a shortcut to open an app of a device: the program is started
/// with `--app <device id> <package>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRequest {
    pub device: String,
    pub package: crate::core::apps::PackageName,
}

/// The app request in the arguments the program was started with (the first is
/// the program itself): exactly `--app`, a device id that is not empty and a
/// package name. Anything else is not one.
pub fn parse_app_args(args: &[String]) -> Option<AppRequest> {
    let [_program, flag, device, package] = args else { return None };
    if flag != "--app" || device.is_empty() {
        return None;
    }
    Some(AppRequest { device: device.clone(), package: crate::core::apps::PackageName::parse(package)? })
}

/// The requests that came together, as one request per action with all its
/// files, in the order the actions first appeared and the files came.
pub fn group(requests: Vec<ShellRequest>) -> Vec<(ShellAction, Vec<PathBuf>)> {
    let mut grouped: Vec<(ShellAction, Vec<PathBuf>)> = Vec::new();
    for request in requests {
        match grouped.iter_mut().find(|(action, _)| *action == request.action) {
            Some((_, files)) => files.push(request.file),
            None => grouped.push((request.action, vec![request.file])),
        }
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn request(action: ShellAction, file: &str) -> ShellRequest {
        ShellRequest { action, file: PathBuf::from(file) }
    }

    #[test]
    fn each_action_has_its_word_and_only_that_word_gives_it_back() {
        for action in [ShellAction::SendDefault, ShellAction::SendChoose, ShellAction::OpenDefault, ShellAction::OpenChoose] {
            assert_eq!(ShellAction::parse(action.word()), Some(action));
        }
        for bad in ["", "send", "Send-Default", "send-default ", "open", "send_default", "--send-default"] {
            assert_eq!(ShellAction::parse(bad), None, "{bad:?}");
        }
        assert!(ShellAction::OpenChoose.opens() && ShellAction::OpenDefault.opens());
        assert!(!ShellAction::SendChoose.opens() && !ShellAction::SendDefault.opens());
        assert!(ShellAction::SendDefault.uses_default() && ShellAction::OpenDefault.uses_default());
        assert!(!ShellAction::SendChoose.uses_default() && !ShellAction::OpenChoose.uses_default());
    }

    #[test]
    fn a_request_is_the_shell_flag_an_action_and_a_file() {
        assert_eq!(
            parse_args(&args(&["scrcpy-agent.exe", "--shell", "send-default", "C:\\x\\a b.txt"])),
            Some(vec![request(ShellAction::SendDefault, "C:\\x\\a b.txt")])
        );
        assert_eq!(
            parse_args(&args(&["app", "--shell", "open-choose", "/tmp/report.pdf"])),
            Some(vec![request(ShellAction::OpenChoose, "/tmp/report.pdf")])
        );
        // A file whose name looks like a flag is still the file.
        assert_eq!(
            parse_args(&args(&["app", "--shell", "send-choose", "--shell"])),
            Some(vec![request(ShellAction::SendChoose, "--shell")])
        );
    }

    #[test]
    fn several_files_are_a_request_for_each_in_the_order_they_came() {
        // The file managers of Linux start the program once for all the selected files.
        assert_eq!(
            parse_args(&args(&["app", "--shell", "open-default", "/tmp/a.txt", "/tmp/b c.pdf", "/tmp/report.doc"])),
            Some(vec![
                request(ShellAction::OpenDefault, "/tmp/a.txt"),
                request(ShellAction::OpenDefault, "/tmp/b c.pdf"),
                request(ShellAction::OpenDefault, "/tmp/report.doc"),
            ])
        );
        // One empty name among them makes it no request: nothing is sent by halves.
        assert_eq!(parse_args(&args(&["app", "--shell", "send-default", "/tmp/a", "", "/tmp/c"])), None);
    }

    #[test]
    fn anything_that_is_not_exactly_a_request_is_not_one() {
        for bad in [
            vec![],
            vec!["app"],
            vec!["app", "--shell"],
            vec!["app", "--shell", "send-default"],
            vec!["app", "--shell", "teleport", "f"],
            vec!["app", "shell", "send-default", "f"],
            vec!["app", "--other", "send-default", "f"],
            vec!["app", "--shell", "send-default", ""],
        ] {
            assert_eq!(parse_args(&args(&bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn an_app_request_is_the_app_flag_a_device_and_a_package() {
        let parsed = parse_app_args(&args(&["scrcpy-agent.exe", "--app", "TESTDEV0001", "com.example.notes"])).unwrap();
        assert_eq!((parsed.device.as_str(), parsed.package.as_str()), ("TESTDEV0001", "com.example.notes"));
        // The device id is whatever it is, spaces and all.
        let wireless = parse_app_args(&args(&["app", "--app", "adb-x._adb-tls-connect._tcp.", "a.b"])).unwrap();
        assert_eq!(wireless.device, "adb-x._adb-tls-connect._tcp.");
        // It is not a file request, and a file request is not an app request.
        assert_eq!(parse_args(&args(&["app", "--app", "TESTDEV", "a.b"])), None);
        assert_eq!(parse_app_args(&args(&["app", "--shell", "send-default", "f"])), None);
    }

    #[test]
    fn anything_that_is_not_exactly_an_app_request_is_not_one() {
        for bad in [
            vec![],
            vec!["app", "--app"],
            vec!["app", "--app", "TESTDEV"],
            vec!["app", "--app", "", "com.example.notes"],
            vec!["app", "--app", "TESTDEV", "nodots"],
            vec!["app", "--app", "TESTDEV", "+com.example.notes"],
            vec!["app", "--app", "TESTDEV", "com.example.notes", "extra"],
            vec!["app", "app", "TESTDEV", "com.example.notes"],
        ] {
            assert_eq!(parse_app_args(&args(&bad)), None, "{bad:?}");
        }
    }

    #[test]
    fn requests_that_came_together_are_one_per_action_in_order() {
        let grouped = group(vec![
            request(ShellAction::SendDefault, "a"),
            request(ShellAction::OpenChoose, "x"),
            request(ShellAction::SendDefault, "b"),
            request(ShellAction::SendDefault, "c"),
            request(ShellAction::OpenChoose, "y"),
        ]);
        assert_eq!(
            grouped,
            [
                (ShellAction::SendDefault, vec![PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")]),
                (ShellAction::OpenChoose, vec![PathBuf::from("x"), PathBuf::from("y")]),
            ]
        );
        assert!(group(Vec::new()).is_empty());
    }
}
