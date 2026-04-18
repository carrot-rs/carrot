//! Linux release packaging: AppImage + deb + rpm.
//!
//! Uses distro-native tools — `cargo-deb`, `cargo-generate-rpm`, and
//! `appimagetool`. Each is optional: the packager skips formats
//! whose tool isn't installed, logging a warning. CI guarantees
//! all three via `apt-get install` / `cargo install` in the
//! release workflow, so "skip" is reserved for local dev boxes.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of a Linux packaging run. Every field is `Option` because
/// individual formats can fall through cleanly when their tool is
/// missing.
pub struct LinuxArtifacts {
    pub deb: Option<PathBuf>,
    pub rpm: Option<PathBuf>,
    pub appimage: Option<PathBuf>,
}

pub fn package(
    workspace: &Path,
    binary_path: &Path,
    version: &str,
    arch: &str,
    output_dir: &Path,
) -> Result<LinuxArtifacts> {
    std::fs::create_dir_all(output_dir)?;
    ensure_desktop_file(
        workspace,
        binary_path.file_name().unwrap().to_str().unwrap(),
    )?;
    let deb = build_deb(workspace, version, arch, output_dir)?;
    let rpm = build_rpm(workspace, version, arch, output_dir)?;
    let appimage = build_appimage(workspace, binary_path, version, arch, output_dir)?;
    Ok(LinuxArtifacts { deb, rpm, appimage })
}

/// `cargo deb` reads `[package.metadata.deb]` from `carrot-app/Cargo.toml`.
fn build_deb(workspace: &Path, version: &str, arch: &str, out: &Path) -> Result<Option<PathBuf>> {
    if !tool_available("cargo-deb") {
        log::warn!("`cargo-deb` not installed — skipping .deb");
        return Ok(None);
    }
    let status = Command::new("cargo")
        .current_dir(workspace)
        .arg("deb")
        .arg("-p")
        .arg("carrot-app")
        .arg("--no-build")
        .arg("--output")
        .arg(out.join(format!("carrot_{version}_{arch}.deb")))
        .status()
        .context("failed to spawn `cargo deb`")?;
    if !status.success() {
        bail!("cargo deb failed with status {status}");
    }
    let path = out.join(format!("carrot_{version}_{arch}.deb"));
    Ok(path.exists().then_some(path))
}

/// `cargo generate-rpm` reads `[package.metadata.generate-rpm]`.
fn build_rpm(workspace: &Path, version: &str, arch: &str, out: &Path) -> Result<Option<PathBuf>> {
    if !tool_available("cargo-generate-rpm") {
        log::warn!("`cargo-generate-rpm` not installed — skipping .rpm");
        return Ok(None);
    }
    let status = Command::new("cargo")
        .current_dir(workspace)
        .arg("generate-rpm")
        .arg("-p")
        .arg("crates/carrot-app")
        .arg("--output")
        .arg(out.join(format!("carrot-{version}.{arch}.rpm")))
        .status()
        .context("failed to spawn `cargo generate-rpm`")?;
    if !status.success() {
        bail!("cargo generate-rpm failed with status {status}");
    }
    let path = out.join(format!("carrot-{version}.{arch}.rpm"));
    Ok(path.exists().then_some(path))
}

/// AppImage via `appimagetool`. Builds an AppDir, copies the binary
/// + desktop file + icon, then runs `appimagetool` to produce the
/// single-file AppImage.
fn build_appimage(
    workspace: &Path,
    binary: &Path,
    version: &str,
    arch: &str,
    out: &Path,
) -> Result<Option<PathBuf>> {
    if !tool_available("appimagetool") {
        log::warn!("`appimagetool` not in PATH — skipping AppImage");
        return Ok(None);
    }
    let appdir = out.join("Carrot.AppDir");
    if appdir.exists() {
        std::fs::remove_dir_all(&appdir).ok();
    }
    std::fs::create_dir_all(appdir.join("usr/bin"))?;
    std::fs::create_dir_all(appdir.join("usr/share/applications"))?;
    std::fs::create_dir_all(appdir.join("usr/share/icons/hicolor/512x512/apps"))?;

    // Binary
    std::fs::copy(binary, appdir.join("usr/bin/carrot"))?;
    std::fs::copy(binary, appdir.join("AppRun"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            appdir.join("AppRun"),
            std::fs::Permissions::from_mode(0o755),
        )?;
    }

    // Desktop file
    let desktop = workspace.join("crates/carrot-app/assets/carrot.desktop");
    std::fs::copy(
        &desktop,
        appdir.join("usr/share/applications/carrot.desktop"),
    )
    .with_context(|| format!("failed to copy {}", desktop.display()))?;
    std::fs::copy(&desktop, appdir.join("carrot.desktop"))?;

    // Icon — fall back to the macOS .icns if no PNG is bundled yet.
    let png = workspace.join("crates/carrot-app/assets/carrot.png");
    if png.exists() {
        std::fs::copy(
            &png,
            appdir.join("usr/share/icons/hicolor/512x512/apps/carrot.png"),
        )?;
        std::fs::copy(&png, appdir.join("carrot.png"))?;
    } else {
        log::warn!(
            "No PNG icon at {} — AppImage will use default icon",
            png.display()
        );
    }

    let output_path = out.join(format!("Carrot-{version}-{arch}.AppImage"));
    let mut cmd = Command::new("appimagetool");
    cmd.arg(&appdir).arg(&output_path);
    // No-fuse mode is friendlier inside containers / GitHub Actions.
    cmd.env("APPIMAGE_EXTRACT_AND_RUN", "1");
    let status = cmd.status().context("failed to spawn `appimagetool`")?;
    if !status.success() {
        bail!("appimagetool failed with status {status}");
    }
    Ok(output_path.exists().then_some(output_path))
}

fn ensure_desktop_file(workspace: &Path, binary_name: &str) -> Result<()> {
    let path = workspace.join("crates/carrot-app/assets/carrot.desktop");
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = format!(
        "[Desktop Entry]\n\
        Type=Application\n\
        Name=Carrot\n\
        GenericName=Terminal Emulator\n\
        Comment=Terminal-first agentic development environment\n\
        Exec={binary_name} %U\n\
        Icon=carrot\n\
        Terminal=false\n\
        Categories=System;TerminalEmulator;Development;\n\
        StartupWMClass=carrot\n"
    );
    std::fs::write(&path, contents)?;
    Ok(())
}

fn tool_available(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
