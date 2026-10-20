//! Reading and writing one JSON file: the only place in the program that does it.
//!
//! A file is written whole under another name and then renamed, so a crash never leaves
//! half of it. A file that is not what it should be is not lost: it is put aside (under
//! the same name with `.broken` after it) before the program goes on with the default.

use super::StoreError;
use serde::{Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

/// What reading a file found.
#[derive(Debug, PartialEq)]
pub enum Read<T> {
    /// There is no file.
    Missing,
    Value(T),
    /// There is a file, and it is not valid for this kind of data.
    Broken,
}

/// What writing a file did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// The file has this content now.
    Changed,
    /// It had it already: nothing was written.
    Unchanged,
}

pub fn read<T: DeserializeOwned>(path: &Path) -> Read<T> {
    // A file that cannot be read at all is taken for one that is not there: there is nothing
    // to put aside, and what is written next either works or says why not.
    let Ok(bytes) = std::fs::read(path) else { return Read::Missing };
    serde_json::from_slice(&bytes).map_or(Read::Broken, Read::Value)
}

/// Writes the value as the content of the file (and the folders above it, if there are none).
pub fn write<T: Serialize>(path: &Path, value: &T) -> Result<Written, StoreError> {
    let io = |what: &str, e: std::io::Error| StoreError::Io(format!("{what} {}: {e}", path.display()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| StoreError::Encode(e.to_string()))?;
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(Written::Unchanged);
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| io("could not make the folder of", e))?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let part = path.with_file_name(format!("{name}.tmp"));
    let result = std::fs::write(&part, &bytes)
        .map_err(|e| io("could not write", e))
        .and_then(|()| std::fs::rename(&part, path).map_err(|e| io("could not replace", e)));
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result.map(|()| Written::Changed)
}

/// Puts a file aside as `<name>.broken` (an older one of that name is replaced); the path it
/// has now, or `None` if it could not be moved.
pub fn set_aside(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let kept = path.with_file_name(format!("{name}.broken"));
    std::fs::rename(path, &kept).ok().map(|()| kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        name: String,
        count: u32,
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample() -> Sample {
        Sample { name: "a b".into(), count: 3 }
    }

    #[test]
    fn what_was_written_is_read_back_and_a_missing_file_is_missing() {
        let dir = scratch("roundtrip");
        let path = dir.join("one.json");
        assert_eq!(read::<Sample>(&path), Read::Missing);
        assert_eq!(write(&path, &sample()).unwrap(), Written::Changed);
        assert_eq!(read::<Sample>(&path), Read::Value(sample()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_makes_the_folders_above_the_file_and_leaves_nothing_of_the_way_it_was_written() {
        let dir = scratch("folders");
        let path = dir.join("a").join("b").join("one.json");
        write(&path, &sample()).unwrap();
        let names: Vec<String> =
            std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["one.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_the_same_content_again_writes_nothing_and_other_content_replaces_it() {
        let dir = scratch("unchanged");
        let path = dir.join("one.json");
        assert_eq!(write(&path, &sample()).unwrap(), Written::Changed);
        assert_eq!(write(&path, &sample()).unwrap(), Written::Unchanged);
        let other = Sample { count: 4, ..sample() };
        assert_eq!(write(&path, &other).unwrap(), Written::Changed);
        assert_eq!(read::<Sample>(&path), Read::Value(other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_valid_for_the_kind_is_broken_not_missing_and_not_an_error() {
        let dir = scratch("broken");
        let path = dir.join("one.json");
        for bad in ["{not json", "", "[1,2]", r#"{"name":"x"}"#, r#"{"name":1,"count":2}"#] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(read::<Sample>(&path), Read::Broken, "{bad:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_put_aside_keeps_its_content_under_another_name_and_replaces_an_older_one() {
        let dir = scratch("aside");
        let path = dir.join("one.json");
        std::fs::write(&path, "first").unwrap();
        let kept = set_aside(&path).unwrap();
        assert_eq!(kept, dir.join("one.json.broken"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "first");
        std::fs::write(&path, "second").unwrap();
        assert_eq!(set_aside(&path).unwrap(), kept);
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "second");
        // A file that is not there cannot be put aside.
        assert_eq!(set_aside(&path), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
