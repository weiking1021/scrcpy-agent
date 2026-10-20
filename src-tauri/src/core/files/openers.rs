//! Opening a file on a device: what kind of file it is, which apps of the device
//! can open that kind, and the commands that ask and that start one. The
//! commands are given to the device's shell, so every value in them is quoted.

#[cfg(test)]
use super::file_types::FileTypes;
use super::transfer::shell_quote;

/// A media type such as `text/plain`: a type and a subtype of letters, digits and
/// the characters `!#$&^_.+-`, joined by `/`, in lower case, each part at most 127
/// characters. A parameter (`; charset=...`) is not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeType(String);

impl MimeType {
    pub fn parse(text: &str) -> Option<Self> {
        let lower = text.to_ascii_lowercase();
        let part_ok = |p: &str| {
            !p.is_empty()
                && p.len() <= 127
                && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"!#$&^_.+-".contains(&b))
        };
        let (kind, subtype) = lower.split_once('/')?;
        (part_ok(kind) && part_ok(subtype)).then_some(MimeType(lower))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The extensions of the common kinds of files and their types (the names
/// registered with IANA, or in common use where there is none).
const TYPES: &[(&str, &str)] = &[
    ("txt", "text/plain"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("css", "text/css"),
    ("csv", "text/csv"),
    ("md", "text/markdown"),
    ("xml", "application/xml"),
    ("json", "application/json"),
    ("pdf", "application/pdf"),
    ("rtf", "application/rtf"),
    ("zip", "application/zip"),
    ("7z", "application/x-7z-compressed"),
    ("rar", "application/vnd.rar"),
    ("gz", "application/gzip"),
    ("tar", "application/x-tar"),
    ("apk", "application/vnd.android.package-archive"),
    ("epub", "application/epub+zip"),
    ("doc", "application/msword"),
    ("docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("xls", "application/vnd.ms-excel"),
    ("xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ("ppt", "application/vnd.ms-powerpoint"),
    ("pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("bmp", "image/bmp"),
    ("svg", "image/svg+xml"),
    ("heic", "image/heic"),
    ("ico", "image/vnd.microsoft.icon"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("flac", "audio/flac"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("mp4", "video/mp4"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("avi", "video/x-msvideo"),
    ("mov", "video/quicktime"),
];

/// The type of a file of this extension (without the dot, any case), from the
/// types that are registered with IANA for the common extensions.
pub fn mime_for_extension(extension: &str) -> Option<MimeType> {
    let lower = extension.to_ascii_lowercase();
    TYPES.iter().find(|(e, _)| *e == lower).and_then(|(_, t)| MimeType::parse(t))
}

/// The name of an activity as Android prints it: `package/class`. Both parts are
/// letters, digits, `_`, `.` and `$`; the package has at least two parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub package: String,
    pub class: String,
}

impl Component {
    pub fn parse(text: &str) -> Option<Self> {
        let (package, class) = text.split_once('/')?;
        let part_ok = |p: &str| {
            let mut chars = p.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        let class_ok = !class.is_empty() && class.chars().all(|c| c.is_ascii_alphanumeric() || "_.$".contains(c));
        (package.split('.').all(part_ok) && class_ok)
            .then(|| Component { package: package.to_string(), class: class.to_string() })
    }
}

/// The activities listed by `cmd package query-activities --brief`: every line
/// that is a component and nothing else.
pub fn parse_handlers(output: &str) -> Vec<Component> {
    output.lines().filter_map(|l| Component::parse(l.trim())).collect()
}

/// What `cmd package resolve-activity --brief` said: the app Android would use,
/// and whether the user made it the default for this kind of file. If the user
/// did not, it is the app that chooses between the others (the chooser).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub component: Component,
    pub is_default: bool,
}

pub fn parse_resolved(output: &str) -> Option<Resolved> {
    let mut lines = output.lines();
    let first = lines.next()?;
    if !first.starts_with("priority=") {
        return None;
    }
    let is_default = first.split(' ').find_map(|token| match token {
        "isDefault=true" => Some(true),
        "isDefault=false" => Some(false),
        _ => None,
    })?;
    let component = Component::parse(lines.next()?.trim())?;
    Some(Resolved { component, is_default })
}

/// The extension of a file when it is plain (see `file_types::plain`), as it is written.
fn plain_extension(path: &std::path::Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    super::file_types::plain(ext).map(|_| ext.to_string())
}

/// The type Windows has registered for the extension (`Content Type` of `.ext`),
/// if it is a media type.
#[cfg(windows)]
fn registered_type(extension: &str) -> Option<MimeType> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CLASSES_ROOT;
    let key = RegKey::predef(HKEY_CLASSES_ROOT).open_subkey(format!(".{extension}")).ok()?;
    MimeType::parse(&key.get_value::<String, _>("Content Type").ok()?)
}

#[cfg(not(windows))]
fn registered_type(_extension: &str) -> Option<MimeType> {
    None
}

/// What kind of file is to be opened, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenType {
    /// The system or the table of common kinds names the type.
    Known(MimeType),
    /// Neither does. The type is not guessed: the device is asked about every app that
    /// can show a file (`*/*`) and decides which opens it, and the user can tell it to
    /// always use one.
    Any,
}

impl OpenType {
    /// The type as the commands to the device write it.
    pub fn as_str(&self) -> &str {
        match self {
            OpenType::Known(mime) => mime.as_str(),
            OpenType::Any => "*/*",
        }
    }
}

/// The type of a file, in this order: the one `registered` (the system) has for its
/// extension, the one an add-on named, the table's; `Any` when none does.
pub fn open_type(
    path: &std::path::Path,
    registered: impl Fn(&str) -> Option<MimeType>,
    addons: &super::file_types::FileTypes,
) -> OpenType {
    let Some(extension) = plain_extension(path) else { return OpenType::Any };
    registered(&extension)
        .or_else(|| addons.get(&extension))
        .or_else(|| mime_for_extension(&extension))
        .map_or(OpenType::Any, OpenType::Known)
}

/// The type of a file, with what Windows has registered for its extension.
pub fn open_type_of_file(path: &std::path::Path, addons: &super::file_types::FileTypes) -> OpenType {
    open_type(path, registered_type, addons)
}

/// The apps that can open the file at `uri` with this type.
///
/// The question has the shape of the start (`start_command`): the address and the
/// type. A question about the type alone makes the device forget the app the user
/// made the default: it keeps the set of apps it listed when the user chose, and
/// clears the choice when it lists a different set (a type alone lists fewer apps
/// than a type with an address). Checked on a phone: after such a question the next
/// start asked again, and after one with the address it did not.
///
/// The address and the type are quoted: the device's shell would read `*/*` as a
/// pattern for files, and a type may have characters it reads (`$`, `&`).
pub fn query_command(uri: &str, ty: &OpenType) -> String {
    format!(
        "cmd package query-activities --brief -a android.intent.action.VIEW -d {} -t {}",
        shell_quote(uri),
        shell_quote(ty.as_str())
    )
}

/// The app the device would start for the file at `uri` (see `query_command` for why it has the address).
pub fn resolve_command(uri: &str, ty: &OpenType) -> String {
    format!(
        "cmd package resolve-activity --brief -a android.intent.action.VIEW -d {} -t {}",
        shell_quote(uri),
        shell_quote(ty.as_str())
    )
}

/// The `file://` address of a path on the device: the characters that are not
/// letters, digits, `-._~` or `/` are written as `%` and their bytes.
pub fn file_uri(path: &str) -> String {
    let mut uri = String::from("file://");
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub fn start_command(uri: &str, ty: &OpenType) -> String {
    format!(
        "am start -a android.intent.action.VIEW -d {} -t {}",
        shell_quote(uri),
        shell_quote(ty.as_str())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mime(t: &str) -> Option<String> {
        MimeType::parse(t).map(|m| m.as_str().to_string())
    }

    #[test]
    fn a_media_type_is_a_type_and_a_subtype_in_lower_case() {
        assert_eq!(mime("text/plain"), Some("text/plain".into()));
        assert_eq!(mime("Application/PDF"), Some("application/pdf".into()));
        assert!(mime("application/vnd.openxmlformats-officedocument.wordprocessingml.document").is_some());
        assert_eq!(mime("image/svg+xml"), Some("image/svg+xml".into()));
        for bad in ["", "text", "text/", "/plain", "text/plain/x", "text/pl ain", "text/plain; charset=utf-8", "te\nxt/plain", "tëxt/plain", "*/*", "text/*"] {
            assert_eq!(mime(bad), None, "{bad:?}");
        }
        assert_eq!(mime(&format!("a/{}", "b".repeat(128))), None);
        assert!(mime(&format!("a/{}", "b".repeat(127))).is_some());
    }

    #[test]
    fn the_type_of_a_common_extension_is_known_in_any_case_and_an_unknown_one_is_not() {
        let known = |e: &str| mime_for_extension(e).map(|m| m.as_str().to_string());
        assert_eq!(known("txt"), Some("text/plain".into()));
        assert_eq!(known("PDF"), Some("application/pdf".into()));
        assert_eq!(known("jpg"), Some("image/jpeg".into()));
        assert_eq!(known("jpeg"), Some("image/jpeg".into()));
        assert_eq!(known("png"), Some("image/png".into()));
        assert_eq!(known("mp4"), Some("video/mp4".into()));
        assert_eq!(known("mp3"), Some("audio/mpeg".into()));
        assert_eq!(known("zip"), Some("application/zip".into()));
        assert_eq!(known("json"), Some("application/json".into()));
        assert_eq!(known("docx"), Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document".into()));
        assert_eq!(known("xyz123"), None);
        assert_eq!(known(""), None);
        assert_eq!(known(".txt"), None, "the dot is not part of the extension");
        assert_eq!(known("tx t"), None);
    }

    fn known(text: &str) -> OpenType {
        OpenType::Known(MimeType::parse(text).unwrap())
    }

    #[test]
    fn a_file_of_a_kind_that_is_known_has_it_and_any_other_is_left_to_the_device_not_given_a_type() {
        // No lookup in the system and no add-on: only the table of common kinds.
        let none = |_: &str| None;
        let addons = FileTypes::new();
        let of = |p: &str| open_type(std::path::Path::new(p), none, &addons);
        assert_eq!(of("/t/a b.txt"), known("text/plain"));
        assert_eq!(of("/t/paper.PDF"), known("application/pdf"));
        // What is not in the table is not given a type for being "probably" something: `.log` is
        // text by use, but the device decides what opens it (and may be told to always use one app).
        for path in ["/t/server.log", "/t/x.zzqq9", "/t/no-extension", "/t/.log", "/t/a.b c", "/t/a.waytoolongextension1"] {
            assert_eq!(of(path), OpenType::Any, "{path}");
        }
    }

    #[test]
    fn what_the_system_has_registered_for_an_extension_comes_before_the_table_and_is_used_for_any_other() {
        let registered = |extension: &str| match extension {
            "txt" => MimeType::parse("text/x-own"),
            "log" => MimeType::parse("text/plain"),
            _ => None,
        };
        let addons = FileTypes::new();
        let of = |p: &str| open_type(std::path::Path::new(p), registered, &addons);
        assert_eq!(of("/t/a.txt"), known("text/x-own"));
        assert_eq!(of("/t/a.log"), known("text/plain"));
        assert_eq!(of("/t/a.pdf"), known("application/pdf"));
        assert_eq!(of("/t/a.zzqq9"), OpenType::Any);
    }

    #[test]
    fn what_an_addon_named_comes_after_the_system_and_before_the_table() {
        let registered = |extension: &str| (extension == "nfo").then(|| MimeType::parse("text/x-system").unwrap());
        let mut addons = FileTypes::new();
        addons.register("viewer", "log", "text/plain").unwrap();
        addons.register("viewer", "pdf", "application/x-better-pdf").unwrap();
        addons.register("viewer", "nfo", "text/x-addon").unwrap();
        let of = |p: &str| open_type(std::path::Path::new(p), registered, &addons);
        // Where the table and the system say nothing, the add-on's word is used...
        assert_eq!(of("/t/server.LOG"), known("text/plain"));
        // ...it corrects the table...
        assert_eq!(of("/t/a.pdf"), known("application/x-better-pdf"));
        // ...but the system's own registration comes first.
        assert_eq!(of("/t/a.nfo"), known("text/x-system"));
        // What nobody names is still left to the device.
        assert_eq!(of("/t/a.zzqq9"), OpenType::Any);
    }

    #[test]
    fn a_type_for_any_app_is_written_as_the_pattern_that_matches_every_type() {
        assert_eq!(OpenType::Any.as_str(), "*/*");
        assert_eq!(known("text/plain").as_str(), "text/plain");
    }

    #[test]
    fn a_component_is_a_package_and_a_class() {
        let parsed = Component::parse("com.example.reader/com.example.reader.ViewActivity").unwrap();
        assert_eq!(parsed.package, "com.example.reader");
        assert_eq!(parsed.class, "com.example.reader.ViewActivity");
        let inner = Component::parse("org.example.app/.Main$Inner").unwrap();
        assert_eq!(inner.class, ".Main$Inner");
        // A single word is a package too: the system's own chooser is `android/...`.
        assert!(Component::parse("android/com.android.internal.app.ResolverActivity").is_some());
        for bad in ["", "a.b/", "/x.Y", "a.b/x y", "a.b/x/y", "a.b", "a..b/X", "1a.b/X", "a.b/X\n"] {
            assert_eq!(Component::parse(bad), None, "{bad:?}");
        }
    }

    // The shape of what the command printed: a count and then one component per
    // line, indented. The names are invented.
    const HANDLERS: &str = "2 activities found:\n  com.example.reader/com.example.reader.ViewActivity\n  com.example.editor/.EditActivity\n";

    #[test]
    fn the_handlers_are_the_lines_that_are_components_and_nothing_else() {
        let handlers = parse_handlers(HANDLERS);
        assert_eq!(handlers.len(), 2);
        assert_eq!(handlers[0].package, "com.example.reader");
        assert_eq!(handlers[1].class, ".EditActivity");
        assert!(parse_handlers("").is_empty());
        assert!(parse_handlers("No activities found\n").is_empty());
        assert!(parse_handlers("Error: something/with a slash\n").is_empty());
    }

    #[test]
    fn what_would_be_used_and_whether_the_user_made_it_the_default_are_read_from_the_two_lines() {
        let chooser = "priority=0 preferredOrder=0 match=0x0 specificIndex=-1 isDefault=false\nandroid/com.android.internal.app.ResolverActivity\n";
        let r = parse_resolved(chooser).unwrap();
        assert!(!r.is_default);
        assert_eq!(r.component, Component { package: "android".into(), class: "com.android.internal.app.ResolverActivity".into() });
        let chosen = "priority=0 preferredOrder=0 match=0x600000 specificIndex=-1 isDefault=true\ncom.example.reader/.ViewActivity\n";
        let r = parse_resolved(chosen).unwrap();
        assert!(r.is_default);
        assert_eq!(r.component.package, "com.example.reader");
        for bad in ["", "isDefault=maybe\ncom.example.reader/.V\n", "priority=0 isDefault=true\nnot a component\n", "priority=0\ncom.example.reader/.V\n"] {
            assert_eq!(parse_resolved(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_commands_quote_what_they_were_given() {
        let m = known("text/plain");
        // The questions have the same shape as the start: with the address of the file. A question about the
        // type alone makes the device forget the app the user made the default (it compares the apps it lists
        // now with the ones it listed when the user chose, and they are not the same set).
        // The type is quoted too: the device's shell would otherwise read `*/*` as a pattern for files,
        // and a type may have characters the shell reads (`$`, `&`).
        assert_eq!(
            query_command("file:///sdcard/Download/a%20b.txt", &m),
            "cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/a%20b.txt' -t 'text/plain'"
        );
        assert_eq!(
            resolve_command("file:///sdcard/Download/a%20b.txt", &m),
            "cmd package resolve-activity --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/a%20b.txt' -t 'text/plain'"
        );
        assert_eq!(
            start_command("file:///sdcard/Download/a%20b.txt", &m),
            "am start -a android.intent.action.VIEW -d 'file:///sdcard/Download/a%20b.txt' -t 'text/plain'"
        );
        assert_eq!(
            start_command("file:///x/it's", &m),
            r"am start -a android.intent.action.VIEW -d 'file:///x/it'\''s' -t 'text/plain'"
        );
        assert_eq!(
            query_command("file:///x/it's", &known("application/x-a$b&c")),
            r"cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///x/it'\''s' -t 'application/x-a$b&c'"
        );
        // Any app: the pattern that matches every type.
        assert_eq!(
            query_command("file:///sdcard/Download/a.log", &OpenType::Any),
            "cmd package query-activities --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/a.log' -t '*/*'"
        );
        assert_eq!(
            resolve_command("file:///sdcard/Download/a.log", &OpenType::Any),
            "cmd package resolve-activity --brief -a android.intent.action.VIEW -d 'file:///sdcard/Download/a.log' -t '*/*'"
        );
        assert_eq!(
            start_command("file:///sdcard/Download/a.log", &OpenType::Any),
            "am start -a android.intent.action.VIEW -d 'file:///sdcard/Download/a.log' -t '*/*'"
        );
    }

    #[test]
    fn a_path_becomes_a_file_address_with_what_is_not_plain_written_as_percent_and_bytes() {
        assert_eq!(file_uri("/sdcard/Download/a.txt"), "file:///sdcard/Download/a.txt");
        assert_eq!(file_uri("/sdcard/Download/a b (1).txt"), "file:///sdcard/Download/a%20b%20%281%29.txt");
        assert_eq!(file_uri("/sdcard/Bär.txt"), "file:///sdcard/B%C3%A4r.txt");
        assert_eq!(file_uri("/a/it's#1?.txt"), "file:///a/it%27s%231%3F.txt");
        assert_eq!(file_uri("/a/~x_y-z.v2"), "file:///a/~x_y-z.v2");
        assert_eq!(file_uri("/a/100%"), "file:///a/100%25");
    }
}
