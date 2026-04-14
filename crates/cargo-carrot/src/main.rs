mod build;
mod bundle;
mod dev;
mod icon;
mod package_linux;
mod package_mac;
mod package_windows;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

/// Cargo subcommand wrapper — `cargo carrot <command>`
#[derive(Parser)]
#[command(name = "cargo", bin_name = "cargo")]
enum Cargo {
    /// Carrot development tools
    Carrot(CarrotCli),
}

#[derive(Parser)]
#[command(version, about = "Carrot development tools")]
struct CarrotCli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run in development mode with hot-reload
    Dev {
        /// Build in release mode
        #[arg(long)]
        release: bool,
    },
    /// Build the application bundle
    Build {
        /// Build in debug mode (default is release)
        #[arg(long)]
        debug: bool,
    },
    /// Compile .icon to Assets.car via actool
    Icon {
        /// Path to .icon file (default: crates/carrot-app/assets/rajin.icon)
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Package the release binary into platform-native installers.
    /// macOS: signed .app + notarized .dmg. Windows: portable .zip
    /// + WiX .msi. Linux: .deb + .rpm + AppImage. Signing steps are
    /// opt-in via environment variables — see each `package_*.rs`
    /// module for the exact secrets they look for.
    Package {
        /// Override the app version (default: read from the
        /// `carrot-app` crate's Cargo.toml).
        #[arg(long)]
        version: Option<String>,
        /// Path to the release binary. Default: walks to
        /// `target/release/carrot[.exe]`.
        #[arg(long)]
        binary: Option<PathBuf>,
        /// macOS-only: second-arch binary used to produce a
        /// Universal build via `lipo`. When unset, the primary
        /// binary is packaged as single-arch.
        #[arg(long)]
        mac_x86_64: Option<PathBuf>,
        /// Linux-only: CPU arch string embedded in the artifact
        /// filename (e.g. `amd64`, `aarch64`). Default `amd64`.
        #[arg(long, default_value = "amd64")]
        linux_arch: String,
        /// Output directory for installer artifacts. Default
        /// `target/release/installers`.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Target platform. `auto` picks from `cfg!(target_os)`.
        #[arg(long, default_value = "auto")]
        platform: Platform,
    },
}

/// Packaging target. `auto` picks the one matching the host OS —
/// cross-packaging isn't supported (macOS needs `codesign`, Windows
/// needs `signtool`, etc.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Platform {
    Auto,
    Macos,
    Windows,
    Linux,
}

impl Platform {
    fn resolve(self) -> Platform {
        match self {
            Platform::Auto => {
                if cfg!(target_os = "macos") {
                    Platform::Macos
                } else if cfg!(target_os = "windows") {
                    Platform::Windows
                } else {
                    Platform::Linux
                }
            }
            other => other,
        }
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp(None)
        .format_target(false)
        .init();

    let Cargo::Carrot(cli) = Cargo::parse();
    let workspace_root = find_workspace_root()?;

    match cli.command {
        Commands::Dev { release } => dev::run(&workspace_root, release),
        Commands::Build { debug } => build::run(&workspace_root, !debug),
        Commands::Icon { path } => icon::run(&workspace_root, path.as_deref()),
        Commands::Package {
            version,
            binary,
            mac_x86_64,
            linux_arch,
            out,
            platform,
        } => run_package(
            &workspace_root,
            version,
            binary,
            mac_x86_64,
            &linux_arch,
            out,
            platform.resolve(),
        ),
    }
}

fn run_package(
    workspace: &PathBuf,
    version_override: Option<String>,
    binary_override: Option<PathBuf>,
    mac_x86_64: Option<PathBuf>,
    linux_arch: &str,
    output_override: Option<PathBuf>,
    platform: Platform,
) -> Result<()> {
    let version = match version_override {
        Some(v) => v,
        None => read_carrot_app_version(workspace)?,
    };
    let release_dir = workspace.join("target/release");
    let binary_default = if cfg!(windows) {
        release_dir.join("carrot.exe")
    } else {
        release_dir.join("carrot")
    };
    let binary = binary_override.unwrap_or(binary_default);
    if !binary.exists() {
        bail!(
            "binary not found at {} — run `cargo carrot build` first \
            or pass --binary",
            binary.display()
        );
    }
    let out = output_override.unwrap_or_else(|| release_dir.join("installers"));
    std::fs::create_dir_all(&out)?;

    match platform {
        Platform::Macos => {
            let packaged_binary = match mac_x86_64 {
                Some(x86) => {
                    let universal = release_dir.join("carrot-universal");
                    package_mac::lipo_universal(&binary, &x86, &universal)?;
                    universal
                }
                None => binary,
            };
            let dmg = package_mac::package(workspace, &packaged_binary, &version, &out)?;
            log::info!("✅ macOS DMG: {}", dmg.display());
        }
        Platform::Windows => {
            let (zip, msi) = package_windows::package(workspace, &binary, &version, &out)?;
            log::info!("✅ Windows ZIP: {}", zip.display());
            if let Some(m) = msi {
                log::info!("✅ Windows MSI: {}", m.display());
            }
        }
        Platform::Linux => {
            let arts = package_linux::package(workspace, &binary, &version, linux_arch, &out)?;
            if let Some(p) = arts.deb {
                log::info!("✅ Linux DEB: {}", p.display());
            }
            if let Some(p) = arts.rpm {
                log::info!("✅ Linux RPM: {}", p.display());
            }
            if let Some(p) = arts.appimage {
                log::info!("✅ Linux AppImage: {}", p.display());
            }
        }
        Platform::Auto => unreachable!("resolve() always picks a concrete platform"),
    }
    Ok(())
}

/// Read the `version = "..."` line out of `crates/carrot-app/Cargo.toml`.
fn read_carrot_app_version(workspace: &Path) -> Result<String> {
    let manifest_path = workspace.join("crates/carrot-app/Cargo.toml");
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("version")
            && let Some(eq) = rest.find('=')
        {
            let value = rest[eq + 1..].trim();
            let stripped = value.trim_matches(['"', '\'']);
            if !stripped.is_empty() {
                return Ok(stripped.to_string());
            }
        }
    }
    bail!("could not parse `version` from {}", manifest_path.display());
}

/// Walk up from CWD to find the workspace root (Cargo.toml with [workspace]).
fn find_workspace_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir().context("failed to get current directory")?;

    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml)
                .with_context(|| format!("failed to read {}", cargo_toml.display()))?;
            if content.contains("[workspace]") {
                return Ok(dir);
            }
        }

        if !dir.pop() {
            bail!("could not find workspace root (no Cargo.toml with [workspace] found)");
        }
    }
}
