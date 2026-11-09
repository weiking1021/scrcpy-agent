//! A self-contained provider is embedded in the Windows executable, so the
//! installer and portable build deploy exactly the same optional integration.
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};
use zip::{ZipWriter, write::SimpleFileOptions};

pub fn build() {
    for input in [
        "Widget.csproj",
        "packages.lock.json",
        "Program.cs",
        "Card.cs",
        "AppxManifest.xml",
        "Assets",
        "icons",
    ] {
        println!("cargo:rerun-if-changed=windows-widget/{input}");
    }
    println!("cargo:rerun-if-env-changed=WINDOWS_WIDGET_PROVIDER");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let rid = match env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "x86_64" => "win-x64",
        "aarch64" => "win-arm64",
        "x86" => "win-x86",
        other => panic!("unsupported widget architecture: {other}"),
    };
    let publish = if let Some(prebuilt) = env::var_os("WINDOWS_WIDGET_PROVIDER") {
        PathBuf::from(prebuilt)
    } else {
        let publish = out.join("widget-provider");
        let status = Command::new("dotnet")
            .args([
                "publish",
                "windows-widget/Widget.csproj",
                "-c",
                "Release",
                "-r",
                rid,
                "--self-contained",
                "true",
                "-o",
            ])
            .arg(&publish)
            .args([
                "-p:EnableWindowsTargeting=true",
                "-p:RestoreLockedMode=true",
                "-p:NuGetAudit=false",
                "-p:DebugType=None",
                "-p:DebugSymbols=false",
            ])
            .arg(format!(
                "-p:Version={}",
                env::var("CARGO_PKG_VERSION").unwrap()
            ))
            .arg(format!(
                "-p:BaseIntermediateOutputPath={}/",
                out.join("widget-obj").display()
            ))
            .arg(format!(
                "-p:BaseOutputPath={}/",
                out.join("widget-bin").display()
            ))
            .status()
            .expect("build the Windows widget provider with the .NET 8 SDK");
        assert!(status.success(), "build the Windows widget provider");
        publish
    };
    assert!(
        publish.join("ScrcpyAgent.Widget.exe").is_file(),
        "the widget provider is missing"
    );
    let archive = out.join("windows-widget.zip");
    let mut zip = ZipWriter::new(fs::File::create(&archive).unwrap());
    pack(&mut zip, &publish, &publish);
    zip.finish().unwrap();
    let hash: String = Sha256::digest(fs::read(archive).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("cargo:rustc-env=WINDOWS_WIDGET_BUILD={}", &hash[..16]);
    println!(
        "cargo:rustc-env=WINDOWS_WIDGET_ARCH={}",
        rid.trim_start_matches("win-")
    );
}

fn pack(zip: &mut ZipWriter<fs::File>, root: &Path, directory: &Path) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            pack(zip, root, &path);
        } else {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            zip.start_file(name, SimpleFileOptions::default()).unwrap();
            zip.write_all(&fs::read(path).unwrap()).unwrap();
        }
    }
}
