//! macOS release packaging: `.app` → codesign → notarize → `.dmg`.
//!
//! Each step is opt-in via environment variables. The workflow stays
//! green even without Apple-Developer credentials — codesign + notarize
//! are skipped silently when the required secrets are missing. That
//! lets CI verify the bundle shape on every PR while keeping signing
//! gated to release tags with the full credential set installed.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bundle::{BundleConfig, create_app_bundle};

/// Opt-in codesigning configuration read from the environment.
pub struct CodesignConfig {
    /// Developer-ID Application certificate CN or SHA-1 hash.
    pub identity: String,
    /// Path to the entitlements.plist. Optional; when `None` a
    /// terminal-friendly default is generated and used.
    pub entitlements: Option<PathBuf>,
}

impl CodesignConfig {
    /// Read `APPLE_SIGNING_IDENTITY` + optional
    /// `APPLE_ENTITLEMENTS_PATH` from the environment.
    pub fn from_env() -> Option<Self> {
        let identity = std::env::var("APPLE_SIGNING_IDENTITY").ok()?;
        if identity.trim().is_empty() {
            return None;
        }
        let entitlements = std::env::var("APPLE_ENTITLEMENTS_PATH")
            .ok()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from);
        Some(Self {
            identity,
            entitlements,
        })
    }
}

/// Opt-in notarization configuration.
pub struct NotarizeConfig {
    /// Apple ID (email) for `notarytool`.
    pub apple_id: String,
    /// App-specific password (not the Apple-ID password).
    pub password: String,
    /// Team ID (10-char string on the Developer account page).
    pub team_id: String,
}

impl NotarizeConfig {
    /// Read `APPLE_ID` + `APPLE_APP_PASSWORD` + `APPLE_TEAM_ID` from the
    /// environment. Returns `None` if any is missing.
    pub fn from_env() -> Option<Self> {
        let apple_id = std::env::var("APPLE_ID").ok()?;
        let password = std::env::var("APPLE_APP_PASSWORD").ok()?;
        let team_id = std::env::var("APPLE_TEAM_ID").ok()?;
        if apple_id.is_empty() || password.is_empty() || team_id.is_empty() {
            return None;
        }
        Some(Self {
            apple_id,
            password,
            team_id,
        })
    }
}

/// Full release pipeline: build bundle → codesign (optional) →
/// notarize (optional) → DMG.
///
/// `binary_path` should be a lipo'd universal binary when targeting
/// Universal macOS — the bundle doesn't care, it just copies.
pub fn package(
    workspace: &Path,
    binary_path: &Path,
    version: &str,
    output_dir: &Path,
) -> Result<PathBuf> {
    let app_path = create_app_bundle(&BundleConfig {
        app_name: "Carrot",
        bundle_id: "life.ollhoff.carrot",
        version,
        binary_path,
        assets_dir: &workspace.join("crates/carrot-app/assets"),
        output_dir,
    })?;

    if let Some(codesign) = CodesignConfig::from_env() {
        codesign_bundle(&app_path, &codesign, workspace)?;
        if let Some(notarize) = NotarizeConfig::from_env() {
            notarize_bundle(&app_path, &notarize)?;
            staple_bundle(&app_path)?;
        } else {
            log::warn!(
                "APPLE_ID / APPLE_APP_PASSWORD / APPLE_TEAM_ID not set — \
                skipping notarization"
            );
        }
    } else {
        log::warn!("APPLE_SIGNING_IDENTITY not set — skipping codesign");
    }

    let dmg = create_dmg(&app_path, output_dir, version)?;
    Ok(dmg)
}

/// Deep-sign every binary inside the bundle with the hardened runtime.
fn codesign_bundle(app: &Path, cfg: &CodesignConfig, workspace: &Path) -> Result<()> {
    let entitlements = cfg
        .entitlements
        .clone()
        .unwrap_or_else(|| default_entitlements_path(workspace));
    ensure_entitlements(&entitlements)?;

    let status = Command::new("codesign")
        .arg("--force")
        .arg("--deep")
        .arg("--options=runtime")
        .arg("--timestamp")
        .arg("--entitlements")
        .arg(&entitlements)
        .arg("--sign")
        .arg(&cfg.identity)
        .arg(app)
        .status()
        .context("failed to spawn `codesign`")?;
    if !status.success() {
        bail!("codesign failed with status {status}");
    }

    // Verify the signature landed.
    let verify = Command::new("codesign")
        .arg("--verify")
        .arg("--strict")
        .arg("--verbose=2")
        .arg(app)
        .status()
        .context("failed to spawn `codesign --verify`")?;
    if !verify.success() {
        bail!("codesign verify failed with status {verify}");
    }
    Ok(())
}

/// Zip the bundle + submit to Apple's notarization service + wait.
fn notarize_bundle(app: &Path, cfg: &NotarizeConfig) -> Result<()> {
    let zip_path = app.with_extension("zip");
    let status = Command::new("ditto")
        .arg("-c")
        .arg("-k")
        .arg("--keepParent")
        .arg(app)
        .arg(&zip_path)
        .status()
        .context("failed to spawn `ditto` for notarization zip")?;
    if !status.success() {
        bail!("ditto zip failed with status {status}");
    }

    let status = Command::new("xcrun")
        .arg("notarytool")
        .arg("submit")
        .arg(&zip_path)
        .arg("--apple-id")
        .arg(&cfg.apple_id)
        .arg("--password")
        .arg(&cfg.password)
        .arg("--team-id")
        .arg(&cfg.team_id)
        .arg("--wait")
        .status()
        .context("failed to spawn `xcrun notarytool`")?;
    std::fs::remove_file(&zip_path).ok();
    if !status.success() {
        bail!("notarytool submit failed with status {status}");
    }
    Ok(())
}

/// Staple the notarization ticket to the bundle so Gatekeeper accepts
/// it offline.
fn staple_bundle(app: &Path) -> Result<()> {
    let status = Command::new("xcrun")
        .arg("stapler")
        .arg("staple")
        .arg(app)
        .status()
        .context("failed to spawn `xcrun stapler`")?;
    if !status.success() {
        bail!("stapler staple failed with status {status}");
    }
    Ok(())
}

/// Build a simple read-only DMG around the bundle via `hdiutil`.
fn create_dmg(app: &Path, output_dir: &Path, version: &str) -> Result<PathBuf> {
    let dmg = output_dir.join(format!("Carrot-{version}.dmg"));
    if dmg.exists() {
        std::fs::remove_file(&dmg).ok();
    }
    let status = Command::new("hdiutil")
        .arg("create")
        .arg("-volname")
        .arg("Carrot")
        .arg("-srcfolder")
        .arg(app)
        .arg("-ov")
        .arg("-format")
        .arg("UDZO")
        .arg(&dmg)
        .status()
        .context("failed to spawn `hdiutil`")?;
    if !status.success() {
        bail!("hdiutil create failed with status {status}");
    }
    log::info!("DMG created at {}", dmg.display());
    Ok(dmg)
}

fn default_entitlements_path(workspace: &Path) -> PathBuf {
    workspace.join("crates/carrot-app/assets/entitlements.plist")
}

fn ensure_entitlements(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(
        path.parent()
            .context("entitlements path has no parent dir")?,
    )?;
    let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.allow-jit</key>
    <true/>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
    <true/>
    <key>com.apple.security.cs.disable-library-validation</key>
    <true/>
    <key>com.apple.security.network.client</key>
    <true/>
    <key>com.apple.security.network.server</key>
    <true/>
    <key>com.apple.security.files.user-selected.read-write</key>
    <true/>
</dict>
</plist>
"#;
    std::fs::write(path, plist)
        .with_context(|| format!("failed to write default entitlements to {}", path.display()))?;
    Ok(())
}

/// Lipo-merge two single-arch binaries into a Universal binary.
pub fn lipo_universal(arm64: &Path, x86_64: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = Command::new("lipo")
        .arg("-create")
        .arg(arm64)
        .arg(x86_64)
        .arg("-output")
        .arg(output)
        .status()
        .context("failed to spawn `lipo`")?;
    if !status.success() {
        bail!("lipo failed with status {status}");
    }
    log::info!("Universal binary at {}", output.display());
    Ok(())
}
