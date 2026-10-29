//! Builds the native Windows menu and its loose identity manifest. No signing.
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

pub fn build() {
    println!("cargo:rerun-if-changed=windows-menu");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let dll = out.join("scrcpy-agent-shell.dll");
    let compiler = cc::Build::new().cpp(true).get_compiler();
    let status = compiler.to_command()
        .args(["/nologo", "/LD", "/MT", "/EHsc", "/std:c++17", "/W4", "/WX", "/DUNICODE", "/D_UNICODE"])
        .arg("windows-menu/menu.cpp")
        .arg(format!("/Fo{}", out.join("windows-menu.obj").display()))
        .args(["/link", "/Brepro", "/DEF:windows-menu/menu.def"])
        .arg(format!("/OUT:{}", dll.display()))
        .arg(format!("/IMPLIB:{}", out.join("windows-menu.lib").display()))
        .args(["shell32.lib", "ole32.lib", "shlwapi.lib", "advapi32.lib", "uuid.lib"])
        .status().expect("start the Windows C++ compiler for the Explorer menu");
    assert!(status.success(), "build the Windows 11 Explorer menu DLL");
    let hash: String = Sha256::digest(fs::read(&dll).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
    let directory = format!("windows11-menu/{}", &hash[..16]);
    println!("cargo:rustc-env=WINDOWS_MENU_DLL_DIR={directory}");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "x64", "aarch64" => "arm64", "x86" => "x86", other => panic!("unsupported Windows architecture: {other}"),
    };
    let manifest = include_str!("AppxManifest.xml")
        .replace("@VERSION@", &format!("{}.0", env::var("CARGO_PKG_VERSION").unwrap()))
        .replace("@ARCH@", arch).replace("@DLL_DIR@", &directory.replace('/', "\\"));
    fs::write(out.join("AppxManifest.xml"), manifest).unwrap();
}
