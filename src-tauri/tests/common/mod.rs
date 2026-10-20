//! What the integration tests share.

use std::path::Path;

/// Puts a program (an example that the tests build) at `dest`, where a test runs it.
///
/// On Windows this is a copy. On Linux a copy fails now and then: while the copy is open
/// for writing, a process that another test thread starts has the file open too until it
/// runs its own program, and a program that is open for writing cannot be run
/// (`ETXTBSY`, "text file busy"). Nothing is opened for writing here: `dest` is a hard
/// link to the program, or, where that is not possible (another file system), to a copy
/// that was made once, before any test that needs it went on (the others wait).
pub fn place_program(src: &Path, dest: &Path) {
    #[cfg(unix)]
    {
        use std::collections::HashMap;
        use std::path::PathBuf;
        use std::sync::Mutex;
        static COPIES: Mutex<Option<HashMap<PathBuf, PathBuf>>> = Mutex::new(None);
        if std::fs::hard_link(src, dest).is_ok() {
            return;
        }
        let mut copies = COPIES.lock().unwrap();
        let copy = copies.get_or_insert_with(HashMap::new).entry(src.to_path_buf()).or_insert_with(|| {
            let dir = std::env::temp_dir().join(format!("scrcpy-agent-programs-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let copy = dir.join(src.file_name().expect("a program has a name"));
            std::fs::copy(src, &copy).unwrap();
            copy
        });
        if std::fs::hard_link(&*copy, dest).is_err() {
            std::fs::copy(&*copy, dest).unwrap();
        }
    }
    #[cfg(not(unix))]
    std::fs::copy(src, dest).unwrap();
}
