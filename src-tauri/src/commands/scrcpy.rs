//! The release of scrcpy in use: what is installed, what can be downloaded and the change of the release.

use crate::Shared;
use crate::core::ScrcpyVersions;
use crate::core::error::CoreError;
use crate::core::scrcpy::release::Offer;
use crate::core::scrcpy::versions::ReleaseTag;
use tauri::State;

/// The releases that are installed, the one in use and the default.
#[tauri::command]
pub fn scrcpy_versions(core: State<Shared>) -> ScrcpyVersions {
    core.scrcpy_versions()
}

/// The newest releases of scrcpy (asks GitHub).
#[tauri::command]
pub async fn scrcpy_available(core: State<'_, Shared>) -> Result<Vec<ReleaseTag>, CoreError> {
    core.scrcpy_available().await
}

/// What there is to download of a release (asks GitHub; downloads nothing).
#[tauri::command]
pub async fn scrcpy_offer(core: State<'_, Shared>, tag: ReleaseTag) -> Result<Offer, CoreError> {
    core.scrcpy_offer(&tag).await
}

/// Downloads a release and uses it; the user has been told what it is.
#[tauri::command]
pub async fn scrcpy_download(core: State<'_, Shared>, tag: ReleaseTag) -> Result<(), CoreError> {
    core.inner().clone().download_scrcpy(tag).await
}

/// Makes an installed release the one in use.
#[tauri::command]
pub async fn scrcpy_use(core: State<'_, Shared>, tag: ReleaseTag) -> Result<(), CoreError> {
    core.inner().clone().use_scrcpy(&tag).await
}
