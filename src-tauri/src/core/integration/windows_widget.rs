//! Optional unsigned widget identity, independent of the Explorer menu.
//! Versioned deployments never overwrite a DLL held by the Widgets Board.
use super::{windows_package, windows11_menu};
use crate::core::error::CoreError;
use std::{collections::BTreeMap, io::Cursor, path::Path, sync::Mutex};

static DEPLOYMENT: Mutex<()> = Mutex::new(());
const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/windows-widget.zip"));
const MANIFEST: &str = include_str!("../../../windows-widget/AppxManifest.xml");
const BUILD: &str = env!("WINDOWS_WIDGET_BUILD");

fn fail(error: impl std::fmt::Display) -> CoreError {
    CoreError::WindowsWidget(error.to_string())
}
fn resource(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    let temporary = path.with_extension("next");
    std::fs::write(&temporary, bytes).map_err(fail)?;
    std::fs::rename(temporary, path).map_err(fail)
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn apply(
    enabled: bool,
    root: &Path,
    texts: &BTreeMap<String, String>,
    pipe: &str,
) -> Result<(), CoreError> {
    if !windows11_menu::available() {
        return if enabled {
            Err(CoreError::Unavailable)
        } else {
            Ok(())
        };
    }
    let _deployment = DEPLOYMENT.lock().unwrap();
    if !enabled {
        if !root.is_dir() {
            return Ok(());
        }
        windows_package::apply(false, root, root, "ScrcpyAgent.Widget").map_err(fail)?;
        return Ok(());
    }
    let directory = root.join(BUILD);
    if !directory.join(".complete").is_file() {
        std::fs::create_dir_all(&directory).map_err(fail)?;
        zip::ZipArchive::new(Cursor::new(PAYLOAD))
            .map_err(fail)?
            .extract(&directory)
            .map_err(fail)?;
        std::fs::write(directory.join(".complete"), BUILD).map_err(fail)?;
    }
    std::fs::create_dir_all(directory.join("Public")).map_err(fail)?;
    let configuration = serde_json::json!({ "executable": std::env::current_exe().map_err(fail)?, "pipe": pipe, "texts": texts });
    resource(
        &directory.join("config.json"),
        &serde_json::to_vec(&configuration).map_err(fail)?,
    )?;
    let preview = if texts
        .get("preview_language")
        .is_some_and(|language| language == "zh-TW")
    {
        "zh-TW"
    } else {
        "en"
    };
    let manifest = MANIFEST
        .replace("@VERSION@", concat!(env!("CARGO_PKG_VERSION"), ".0"))
        .replace("@ARCH@", env!("WINDOWS_WIDGET_ARCH"))
        .replace(
            "@APP_NAME@",
            &xml(texts
                .get("app_name")
                .map(String::as_str)
                .unwrap_or("ScrcpyAgent")),
        )
        .replace(
            "@NAME@",
            &xml(texts
                .get("name")
                .map(String::as_str)
                .unwrap_or("Connected devices")),
        )
        .replace(
            "@DESCRIPTION@",
            &xml(texts
                .get("description")
                .map(String::as_str)
                .unwrap_or("Connected devices and quick actions")),
        )
        .replace("@PREVIEW_LANGUAGE@", preview);
    resource(&directory.join("AppxManifest.xml"), manifest.as_bytes())?;
    windows_package::apply(true, &directory, &directory, "ScrcpyAgent.Widget").map_err(fail)?;
    Ok(())
}
