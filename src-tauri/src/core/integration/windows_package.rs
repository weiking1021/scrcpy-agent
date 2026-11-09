//! Serializes unsigned Windows identity registration, including the temporary
//! Developer Mode change shared by the Explorer menu and widget provider.

use std::{path::Path, process::Command, sync::Mutex};

static REGISTRATION: Mutex<()> = Mutex::new(());
const REGISTER: &str = include_str!("../../../windows-package/register.ps1");

pub fn apply(enabled: bool, directory: &Path, external: &Path, name: &str) -> Result<(), String> {
    let _registration = REGISTRATION.lock().unwrap();
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let script = directory.join("register.ps1");
    if !std::fs::read(&script).is_ok_and(|old| old == REGISTER.as_bytes()) {
        std::fs::write(&script, REGISTER).map_err(|e| e.to_string())?;
    }
    let system = std::env::var_os("SystemRoot").ok_or("SystemRoot is not set")?;
    let powershell = Path::new(&system).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    use std::os::windows::process::CommandExt;
    let output = Command::new(powershell)
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(["-Action", if enabled { "Enable" } else { "Disable" }])
        .arg("-ManifestPath")
        .arg(directory.join("AppxManifest.xml"))
        .arg("-ExternalLocation")
        .arg(external)
        .args(["-PackageName", name])
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
