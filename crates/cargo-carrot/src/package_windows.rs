//! Windows release packaging: portable ZIP + WiX MSI installer.
//!
//! MSI signing is opt-in via `WINDOWS_CERT_THUMBPRINT` (SHA-1 of the
//! code-signing certificate installed in the runner's certificate
//! store). CI without the cert builds an unsigned MSI — Windows
//! SmartScreen will warn on first run but installation still works.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a portable ZIP alongside a WiX MSI.
///
/// Returns `(zip_path, msi_path)`.
pub fn package(
    workspace: &Path,
    binary_path: &Path,
    version: &str,
    output_dir: &Path,
) -> Result<(PathBuf, Option<PathBuf>)> {
    std::fs::create_dir_all(output_dir)?;
    let zip = create_portable_zip(binary_path, version, output_dir)?;
    let msi = create_msi(workspace, binary_path, version, output_dir)?;

    if let Some(ref msi_path) = msi
        && let Ok(thumbprint) = std::env::var("WINDOWS_CERT_THUMBPRINT")
        && !thumbprint.trim().is_empty()
    {
        signtool_sign(msi_path, &thumbprint)?;
    } else {
        log::warn!("WINDOWS_CERT_THUMBPRINT not set — MSI is unsigned");
    }

    Ok((zip, msi))
}

/// Zip the binary into `Carrot-<version>-x86_64.zip`. Pure portable
/// distribution, no installer needed.
fn create_portable_zip(binary: &Path, version: &str, out: &Path) -> Result<PathBuf> {
    let zip_path = out.join(format!("Carrot-{version}-x86_64.zip"));
    if zip_path.exists() {
        std::fs::remove_file(&zip_path).ok();
    }
    // Use PowerShell's `Compress-Archive` — it ships with every
    // Windows runner, no extra tool to install.
    #[cfg(windows)]
    {
        let status = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(format!(
                "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                binary.display(),
                zip_path.display(),
            ))
            .status()
            .context("failed to spawn powershell for Compress-Archive")?;
        if !status.success() {
            bail!("Compress-Archive failed with status {status}");
        }
    }
    #[cfg(not(windows))]
    {
        // Cross-platform fallback: `zip` if available.
        let status = Command::new("zip")
            .arg("-j")
            .arg(&zip_path)
            .arg(binary)
            .status()
            .context("failed to spawn `zip`")?;
        if !status.success() {
            bail!("zip failed with status {status}");
        }
    }
    log::info!("Portable ZIP at {}", zip_path.display());
    Ok(zip_path)
}

/// Build an MSI via `cargo-wix`. Returns `None` when `cargo-wix`
/// isn't installed — the caller logs and continues so CI without
/// WiX still yields the portable zip.
fn create_msi(
    _workspace: &Path,
    binary: &Path,
    version: &str,
    out: &Path,
) -> Result<Option<PathBuf>> {
    let wix_available = Command::new("cargo")
        .arg("wix")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !wix_available {
        log::warn!("`cargo wix` not available — skipping MSI build");
        return Ok(None);
    }

    // `cargo wix` reads `wix/main.wxs` next to Cargo.toml. Generate
    // a minimal manifest if the repo doesn't ship one yet — the
    // consumer can customise later.
    let wix_dir = binary
        .parent()
        .and_then(|p| p.parent())
        .context("cannot locate workspace from binary path")?
        .join("wix");
    ensure_wix_manifest(&wix_dir, version)?;

    let status = Command::new("cargo")
        .arg("wix")
        .arg("--nocapture")
        .arg("--install-version")
        .arg(version)
        .arg("--output")
        .arg(out.join(format!("Carrot-{version}-x86_64.msi")))
        .status()
        .context("failed to spawn `cargo wix`")?;
    if !status.success() {
        bail!("cargo wix failed with status {status}");
    }

    let msi = out.join(format!("Carrot-{version}-x86_64.msi"));
    if !msi.exists() {
        bail!("expected MSI at {} not produced", msi.display());
    }
    log::info!("MSI at {}", msi.display());
    Ok(Some(msi))
}

fn ensure_wix_manifest(wix_dir: &Path, version: &str) -> Result<()> {
    if wix_dir.join("main.wxs").exists() {
        return Ok(());
    }
    std::fs::create_dir_all(wix_dir)?;
    let manifest = format!(
        r#"<?xml version='1.0' encoding='windows-1252'?>
<Wix xmlns='http://schemas.microsoft.com/wix/2006/wi'>
    <Product Name='Carrot' Manufacturer='Nyxb' Id='*' UpgradeCode='7F9B12E4-3C4D-4B8F-AF30-8A1B91234567' Language='1033' Codepage='1252' Version='{version}'>
        <Package Id='*' Keywords='Installer' Description='Carrot Terminal' Manufacturer='Nyxb' InstallerVersion='450' Languages='1033' Compressed='yes' SummaryCodepage='1252' InstallScope='perUser'/>
        <Media Id='1' Cabinet='carrot.cab' EmbedCab='yes'/>
        <Directory Id='TARGETDIR' Name='SourceDir'>
            <Directory Id='LocalAppDataFolder'>
                <Directory Id='INSTALLDIR' Name='Carrot'>
                    <Component Id='MainExecutable' Guid='*'>
                        <File Id='carrotEXE' Name='carrot.exe' DiskId='1' Source='target\release\carrot.exe' KeyPath='yes'/>
                    </Component>
                </Directory>
            </Directory>
        </Directory>
        <Feature Id='Complete' Level='1'>
            <ComponentRef Id='MainExecutable'/>
        </Feature>
    </Product>
</Wix>
"#,
    );
    std::fs::write(wix_dir.join("main.wxs"), manifest)?;
    Ok(())
}

/// Sign the MSI via Windows `signtool`. Thumbprint references a
/// certificate already in the runner's store (CI imports it from a
/// `.pfx` blob in secrets via `certutil`).
fn signtool_sign(msi: &Path, thumbprint: &str) -> Result<()> {
    let status = Command::new("signtool")
        .arg("sign")
        .arg("/sha1")
        .arg(thumbprint)
        .arg("/tr")
        .arg("http://timestamp.digicert.com")
        .arg("/td")
        .arg("sha256")
        .arg("/fd")
        .arg("sha256")
        .arg(msi)
        .status()
        .context("failed to spawn `signtool`")?;
    if !status.success() {
        bail!("signtool sign failed with status {status}");
    }
    Ok(())
}
