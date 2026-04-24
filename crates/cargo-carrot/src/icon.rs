use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// Compile .icon to Assets.car via actool.
pub fn run(workspace_root: &Path, icon_path: Option<&Path>) -> Result<()> {
    let default_icon = workspace_root.join("assets/icons/carrot.icon");
    let icon_path = icon_path.unwrap_or(&default_icon);

    if !icon_path.exists() {
        bail!("icon file not found at {}", icon_path.display());
    }

    let output_dir = workspace_root.join("crates/carrot-app/assets");
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    compile_assets_car(icon_path, &output_dir)?;

    log::info!("Icon assets written to {}", output_dir.display());
    Ok(())
}

/// Compile .icon → Assets.car via actool.
fn compile_assets_car(icon_path: &Path, output_dir: &Path) -> Result<()> {
    let plist_path = std::env::temp_dir().join("carrot_icon_plist.plist");

    log::info!("Compiling {} → Assets.car", icon_path.display());

    let status = Command::new("actool")
        .arg(icon_path)
        .arg("--compile")
        .arg(output_dir)
        .arg("--output-format")
        .arg("human-readable-text")
        .arg("--notices")
        .arg("--warnings")
        .arg("--errors")
        .arg("--output-partial-info-plist")
        .arg(&plist_path)
        .arg("--app-icon")
        .arg("carrot")
        .arg("--include-all-app-icons")
        .arg("--enable-on-demand-resources")
        .arg("NO")
        .arg("--development-region")
        .arg("en")
        .arg("--target-device")
        .arg("mac")
        .arg("--minimum-deployment-target")
        .arg("26.0")
        .arg("--platform")
        .arg("macosx")
        .status()
        .context("failed to run actool — is Xcode 26+ installed?")?;

    if !status.success() {
        bail!("actool failed");
    }

    let _ = std::fs::remove_file(&plist_path);

    // actool on macOS 26 writes a legacy .icns alongside Assets.car —
    // carrot targets macOS 26+ exclusively and ships only Assets.car.
    let _ = std::fs::remove_file(output_dir.join("carrot.icns"));

    log::info!("Assets.car compiled");
    Ok(())
}
