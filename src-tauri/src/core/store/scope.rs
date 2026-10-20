//! A place in the data folder that holds kinds of data, and how they are asked for.
//!
//! A kind of data (`kinds`) is a type: it says which scope it belongs to, what its value is,
//! and the name of its file. Nothing else names a file: a part of the program asks its
//! scope for `load::<Shortcuts>()` and gets a value, so no path is built (or can be built
//! wrong) anywhere but here, and a kind of a device cannot be asked of the settings.

use super::StoreError;
use super::file::{self, Read, Written};
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;
use std::path::PathBuf;

/// What a scope is for.
pub trait ScopeKind {}

/// The program's own data.
pub struct Global;
/// The data of one device.
pub struct Device;
/// The data of an add-on (alone, or for one device).
pub struct Addon;

impl ScopeKind for Global {}
impl ScopeKind for Device {}
impl ScopeKind for Addon {}

/// A kind of data: the scope it is in, its value, and the name of its file (without `.json`).
pub trait Data {
    type Scope: ScopeKind;
    /// What is used when there is no file, or the file is not valid.
    type Value: Default + Serialize + DeserializeOwned;
    const NAME: &'static str;
}

/// What is wrong with a file that was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// The file is not valid for this kind of data; it was put aside (here) and the default is used.
    Broken { kept_as: Option<PathBuf> },
}

/// A value read, with what was wrong with the file it came from, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded<V> {
    pub value: V,
    pub problem: Option<Problem>,
}

/// A folder and the kinds of data of its scope.
#[derive(Debug, Clone)]
pub struct Scope<S: ScopeKind> {
    dir: PathBuf,
    scope: PhantomData<S>,
}

impl<S: ScopeKind> Scope<S> {
    pub(super) fn new(dir: PathBuf) -> Self {
        Scope { dir, scope: PhantomData }
    }

    fn file_of<K: Data<Scope = S>>(&self) -> PathBuf {
        self.dir.join(format!("{}.json", K::NAME))
    }

    /// The value of a kind of data: the default when there is none or it is not valid.
    pub fn load<K: Data<Scope = S>>(&self) -> K::Value {
        self.load_checked::<K>().value
    }

    /// The same, with what was wrong with the file (a file that is not valid is put aside, so
    /// that the next write does not take it away).
    pub fn load_checked<K: Data<Scope = S>>(&self) -> Loaded<K::Value> {
        let path = self.file_of::<K>();
        match file::read::<K::Value>(&path) {
            Read::Value(value) => Loaded { value, problem: None },
            Read::Missing => Loaded { value: K::Value::default(), problem: None },
            Read::Broken => {
                Loaded { value: K::Value::default(), problem: Some(Problem::Broken { kept_as: file::set_aside(&path) }) }
            }
        }
    }

    /// Whether there is a file of this kind (valid or not).
    pub fn exists<K: Data<Scope = S>>(&self) -> bool {
        self.file_of::<K>().is_file()
    }

    /// Writes the value; a value that is the content already is not written again.
    pub fn save<K: Data<Scope = S>>(&self, value: &K::Value) -> Result<Written, StoreError> {
        file::write(&self.file_of::<K>(), value)
    }

    /// Takes the data of this kind away; there being none is not an error.
    pub fn remove<K: Data<Scope = S>>(&self) {
        let _ = std::fs::remove_file(self.file_of::<K>());
    }

    fn file_named(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }
}

/// The name of a value an add-on keeps: lower or upper case letters, digits, `_` and `-` (the
/// same words as the names of add-ons and of their entries), so it cannot name another place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataName(String);

impl DataName {
    pub fn new(name: &str) -> Option<DataName> {
        crate::core::addons::buttons::valid_name(name).then(|| DataName(name.to_string()))
    }
}

impl Scope<Device> {
    /// The place of an add-on in the folder of this device; `None` for a name that is not one.
    pub fn addon(&self, name: &str) -> Option<Scope<Addon>> {
        crate::core::addons::buttons::valid_name(name).then(|| Scope::new(self.dir.join("addons").join(name)))
    }
}

impl Scope<Addon> {
    /// A value an add-on kept (any JSON); `null` when there is none or it is not valid (a file that
    /// is not valid is put aside, as with the kinds of the program).
    pub fn load_value(&self, name: &DataName) -> serde_json::Value {
        let path = self.file_named(&name.0);
        match file::read::<serde_json::Value>(&path) {
            Read::Value(value) => value,
            Read::Missing => serde_json::Value::Null,
            Read::Broken => {
                let _ = file::set_aside(&path);
                serde_json::Value::Null
            }
        }
    }

    pub fn save_value(&self, name: &DataName, value: &serde_json::Value) -> Result<Written, StoreError> {
        file::write(&self.file_named(&name.0), value)
    }

    pub fn remove_value(&self, name: &DataName) {
        let _ = std::fs::remove_file(self.file_named(&name.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Note {
        text: String,
    }

    struct Memo;
    impl Data for Memo {
        type Scope = Global;
        type Value = Note;
        const NAME: &'static str = "memo";
    }

    struct Other;
    impl Data for Other {
        type Scope = Global;
        type Value = Note;
        const NAME: &'static str = "other";
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scrcpy-agent-scope-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn note(text: &str) -> Note {
        Note { text: text.into() }
    }

    #[test]
    fn a_kind_is_a_file_of_its_name_in_the_folder_of_its_scope_and_gives_its_value_back() {
        let dir = scratch("roundtrip");
        let scope = Scope::<Global>::new(dir.clone());
        assert!(!scope.exists::<Memo>());
        assert_eq!(scope.load::<Memo>(), Note::default());
        assert_eq!(scope.save::<Memo>(&note("a")).unwrap(), Written::Changed);
        assert!(scope.exists::<Memo>());
        assert!(dir.join("memo.json").is_file());
        assert_eq!(scope.load::<Memo>(), note("a"));
        // Another kind of the scope is another file.
        assert_eq!(scope.load::<Other>(), Note::default());
        scope.save::<Other>(&note("b")).unwrap();
        assert_eq!(scope.load::<Memo>(), note("a"));
        assert_eq!(scope.load::<Other>(), note("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_value_that_is_the_content_already_is_not_written_again() {
        let dir = scratch("unchanged");
        let scope = Scope::<Global>::new(dir.clone());
        assert_eq!(scope.save::<Memo>(&note("a")).unwrap(), Written::Changed);
        assert_eq!(scope.save::<Memo>(&note("a")).unwrap(), Written::Unchanged);
        assert_eq!(scope.save::<Memo>(&note("b")).unwrap(), Written::Changed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_valid_is_put_aside_and_the_default_is_used_and_the_problem_is_told() {
        let dir = scratch("broken");
        let scope = Scope::<Global>::new(dir.clone());
        std::fs::write(dir.join("memo.json"), "{not json").unwrap();
        let loaded = scope.load_checked::<Memo>();
        assert_eq!(loaded.value, Note::default());
        assert_eq!(loaded.problem, Some(Problem::Broken { kept_as: Some(dir.join("memo.json.broken")) }));
        assert_eq!(std::fs::read_to_string(dir.join("memo.json.broken")).unwrap(), "{not json");
        // The next write does not take the broken content away, and a read finds nothing wrong.
        assert!(!scope.exists::<Memo>());
        scope.save::<Memo>(&note("fresh")).unwrap();
        assert_eq!(scope.load_checked::<Memo>(), Loaded { value: note("fresh"), problem: None });
        assert_eq!(std::fs::read_to_string(dir.join("memo.json.broken")).unwrap(), "{not json");
        // `load` says nothing of it, and does the same.
        std::fs::write(dir.join("other.json"), "[1]").unwrap();
        assert_eq!(scope.load::<Other>(), Note::default());
        assert!(dir.join("other.json.broken").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn taking_the_data_away_removes_the_file_and_may_be_done_twice() {
        let dir = scratch("remove");
        let scope = Scope::<Global>::new(dir.clone());
        scope.save::<Memo>(&note("a")).unwrap();
        scope.save::<Other>(&note("b")).unwrap();
        scope.remove::<Memo>();
        scope.remove::<Memo>();
        assert!(!scope.exists::<Memo>());
        assert_eq!(scope.load::<Other>(), note("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
