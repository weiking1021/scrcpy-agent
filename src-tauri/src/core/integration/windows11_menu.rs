//! The Windows 11 file menu has its own switch. A small native COM DLL supplies
//! the entries; it reads only the executable and translated labels below CONFIG.
//! The sparse package gives the extension an identity without packaging the app.

use super::explorer_menu::MenuTexts;
use crate::core::error::CoreError;
use std::{path::Path, process::Command, sync::Mutex};
use winreg::{
    RegKey,
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ},
};

const CONFIG: &str = r"Software\scrcpy-agent\Windows11Menu";
const DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scrcpy-agent-shell.dll"));
const MANIFEST: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/AppxManifest.xml"));
const REGISTER: &str = include_str!("../../../windows-menu/register.ps1");
// Startup, a setting change and a language change must not deploy concurrently.
static DEPLOYMENT: Mutex<()> = Mutex::new(());

pub fn available() -> bool {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ)
        .and_then(|key| key.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|build| build.parse::<u32>().ok())
        .is_some_and(|build| build >= 22000)
}

fn fail(error: impl std::fmt::Display) -> CoreError {
    CoreError::ExplorerMenu(error.to_string())
}

/// Only rewrites a resource when it changed. In particular, never opens a DLL
/// already mapped by Explorer for writing. Different builds have different paths.
fn resource(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    std::fs::write(path, bytes).map_err(fail)
}

pub fn apply(enabled: bool, directory: &Path, texts: &MenuTexts) -> Result<(), CoreError> {
    if !available() {
        return if enabled { Err(CoreError::Unavailable) } else { Ok(()) };
    }
    let _deployment = DEPLOYMENT.lock().unwrap();
    let user = RegKey::predef(HKEY_CURRENT_USER);
    // An untouched installation has nothing to remove and need not start PowerShell.
    if !enabled && !directory.join("register.ps1").is_file() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(fail)?;
    let external = exe
        .parent()
        .ok_or_else(|| fail("the application has no parent folder"))?;
    if enabled {
        resource(
            &external
                .join(env!("WINDOWS_MENU_DLL_DIR"))
                .join("scrcpy-agent-shell.dll"),
            DLL,
        )?;
        resource(&directory.join("AppxManifest.xml"), MANIFEST)?;
        for (name, bytes) in [
            ("StoreLogo.png", include_bytes!("../../../icons/StoreLogo.png").as_slice()),
            ("Square150x150Logo.png", include_bytes!("../../../icons/Square150x150Logo.png").as_slice()),
            ("Square44x44Logo.png", include_bytes!("../../../icons/Square44x44Logo.png").as_slice()),
        ] { resource(&directory.join("Assets").join(name), bytes)?; }
    }
    resource(&directory.join("register.ps1"), REGISTER.as_bytes())?;

    // Closing or hiding a menu is immediate even while Windows unregisters its
    // identity. User data and the older Explorer menu have separate locations.
    if !enabled {
        match user.delete_subkey_all(CONFIG) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(fail(e)),
            _ => {}
        }
    }
    let system = std::env::var_os("SystemRoot").ok_or_else(|| fail("SystemRoot is not set"))?;
    let powershell = Path::new(&system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(directory.join("register.ps1"))
        .arg("-Action")
        .arg(if enabled { "Enable" } else { "Disable" })
        .arg("-ManifestPath")
        .arg(directory.join("AppxManifest.xml"))
        .arg("-ExternalLocation")
        .arg(external);
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let output = command.output().map_err(fail)?;
    if !output.status.success() {
        // Restore visibility when disabling failed, since the setting is rolled back.
        if !enabled {
            write_config(&exe, texts)?;
        }
        return Err(fail(String::from_utf8_lossy(&output.stderr).trim()));
    }
    if enabled {
        write_config(&exe, texts)?;
    }
    Ok(())
}

fn write_config(exe: &Path, texts: &MenuTexts) -> Result<(), CoreError> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(CONFIG).map_err(fail)?;
    for (name, value) in [
        ("Title", texts.title.as_str()),
        ("SendDefault", &texts.send_default),
        ("SendChoose", &texts.send_choose),
        ("OpenDefault", &texts.open_default),
        ("OpenChoose", &texts.open_choose),
    ] {
        key.set_value(name, &value).map_err(fail)?;
    }
    key.set_value("Executable", &exe.to_string_lossy().as_ref())
        .map_err(fail)
}
