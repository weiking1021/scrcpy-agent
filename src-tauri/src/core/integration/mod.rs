//! How the application appears in the operating system: menus, shortcuts and
//! launcher entries.

pub mod autostart;
pub mod desktop_entry;
pub mod explorer_menu;
pub mod file_manager_menu;
pub mod shortcuts;
#[cfg(windows)]
pub mod windows11_menu;
