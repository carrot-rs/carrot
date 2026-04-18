use anyhow::{Context, Result, bail};
use cargo_metadata::{MetadataCommand, Package, PackageId};
use notify_debouncer_mini::{
    Debouncer, new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
};
use shared_child::SharedChild;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::bundle::{self, BundleConfig};

const ROOT_PACKAGE: &str = "carrot-app";

/// Run in development mode: build, bundle, launch, watch, rebuild.
pub fn run(workspace_root: &Path, release: bool) -> Result<()> {
    let profile = if release { "release" } else { "debug" };
    let target_dir = workspace_root.join("target").join(profile);
    let binary_path = target_dir.join("carrot");
    let assets_dir = workspace_root.join("crates/carrot-app/assets");

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .context("failed to set Ctrl+C handler")?;

    if !cargo_build(workspace_root, release)? {
        bail!("initial build failed");
    }

    let bundle_path = bundle::create_app_bundle(&BundleConfig {
        app_name: "Carrot",
        bundle_id: "dev.nyxb.carrot",
        version: "0.1.0",
        binary_path: &binary_path,
        assets_dir: &assets_dir,
        output_dir: &target_dir,
    })?;

    let mut child = launch_app(&bundle_path)?;

    let mut watch_roots = resolve_workspace_dependency_paths(workspace_root)
        .context("failed to resolve workspace dependency paths")?;
    let workspace_assets = workspace_root.join("assets");
    if workspace_assets.exists() {
        watch_roots.push(workspace_assets);
    }

    let (tx, rx) = mpsc::channel();
    let mut debouncer =
        new_debouncer(Duration::from_secs(1), tx).context("failed to create file watcher")?;

    let mut watched_entries = 0usize;
    for root in &watch_roots {
        watched_entries += install_watches(&mut debouncer, root)?;
    }
    install_workspace_manifest_watch(&mut debouncer, workspace_root)?;

    log::info!(
        "👁  Watching {} crate roots ({} watch entries) for changes...",
        watch_roots.len(),
        watched_entries
    );

    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Ok(events)) => {
                let relevant_change = events.iter().any(|e| is_relevant_change(&e.path));
                if !relevant_change {
                    continue;
                }

                log::info!("🔄 Change detected, rebuilding...");

                kill_process_tree(&child);

                if cargo_build(workspace_root, release)? {
                    let _ = bundle::create_app_bundle(&BundleConfig {
                        app_name: "Carrot",
                        bundle_id: "dev.nyxb.carrot",
                        version: "0.1.0",
                        binary_path: &binary_path,
                        assets_dir: &assets_dir,
                        output_dir: &target_dir,
                    });

                    child = launch_app(&bundle_path)?;
                    log::info!("👁  Watching for changes...");
                } else {
                    log::error!("Build failed, waiting for next change...");
                }
            }
            Ok(Err(err)) => {
                log::warn!("watch error: {err}");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    log::info!("Shutting down...");
    kill_process_tree(&child);

    Ok(())
}

/// Install watches for everything one level deep inside `root`, gitignore-aware.
/// Sub-directories are watched recursively, individual files non-recursively
/// — mirrors Tauri's `lookup` helper. Returns the number of watch handles
/// installed so the caller can report it.
fn install_watches(debouncer: &mut Debouncer<RecommendedWatcher>, root: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in ignore::WalkBuilder::new(root)
        .require_git(false)
        .max_depth(Some(1))
        .build()
        .flatten()
    {
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.into_path();
        if path == root {
            continue;
        }
        let mode = if file_type.is_dir() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        debouncer
            .watcher()
            .watch(&path, mode)
            .with_context(|| format!("failed to watch {}", path.display()))?;
        count += 1;
    }
    Ok(count)
}

/// Watch the workspace `Cargo.toml` and `Cargo.lock` directly. Walking the
/// workspace root would also pick up every other top-level entry (incl. the
/// full `crates/` tree), which we don't want — crate-root selection is driven
/// by `cargo metadata`.
fn install_workspace_manifest_watch(
    debouncer: &mut Debouncer<RecommendedWatcher>,
    workspace_root: &Path,
) -> Result<()> {
    for name in ["Cargo.toml", "Cargo.lock"] {
        let path = workspace_root.join(name);
        if path.exists() {
            debouncer
                .watcher()
                .watch(&path, RecursiveMode::NonRecursive)
                .with_context(|| format!("failed to watch {}", path.display()))?;
        }
    }
    Ok(())
}

/// Resolve all workspace crate directories that `carrot-app` transitively
/// depends on via `path = "..."` dependencies. Mirrors the Tauri CLI approach
/// (`get_in_workspace_dependency_paths`) so any source change inside a relevant
/// crate triggers a rebuild — instead of the previous hardcoded list.
fn resolve_workspace_dependency_paths(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    let metadata = MetadataCommand::new()
        .manifest_path(workspace_root.join("Cargo.toml"))
        .exec()
        .context("failed to query cargo metadata")?;

    let workspace_packages: Vec<&Package> = metadata
        .packages
        .iter()
        .filter(|p| metadata.workspace_members.contains(&p.id))
        .collect();

    let root_pkg = workspace_packages
        .iter()
        .copied()
        .find(|p| p.name.as_str() == ROOT_PACKAGE)
        .with_context(|| format!("{ROOT_PACKAGE} not found in workspace members"))?;

    let mut visited: HashSet<PackageId> = HashSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_path_deps(root_pkg, &workspace_packages, &mut visited, &mut paths);
    Ok(paths)
}

fn collect_path_deps(
    package: &Package,
    workspace_packages: &[&Package],
    visited: &mut HashSet<PackageId>,
    paths: &mut Vec<PathBuf>,
) {
    if !visited.insert(package.id.clone()) {
        return;
    }
    if let Some(parent) = package.manifest_path.parent() {
        paths.push(parent.as_std_path().to_path_buf());
    }
    for dep in &package.dependencies {
        let Some(dep_path) = &dep.path else { continue };
        let Some(dep_pkg) = workspace_packages.iter().copied().find(|p| {
            p.name == dep.name
                && p.manifest_path.parent().map(|d| d.as_std_path()) == Some(dep_path.as_std_path())
        }) else {
            continue;
        };
        collect_path_deps(dep_pkg, workspace_packages, visited, paths);
    }
}

/// Filter for events that should trigger a rebuild. Source code, manifests,
/// and bundled config (themes, completion specs, keymaps) matter; lockfile
/// churn from cargo's own writes, build artifacts, and editor scratch files
/// do not.
fn is_relevant_change(path: &Path) -> bool {
    if path.components().any(|c| c.as_os_str() == "target") {
        return false;
    }
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    if name.starts_with('.') || name.ends_with('~') {
        return false;
    }
    // Cargo.lock is rewritten by cargo itself during build, which would feed
    // back into the watcher and trigger a no-op rebuild loop.
    if name == "Cargo.lock" {
        return false;
    }
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    matches!(ext, "rs" | "toml" | "json")
}

fn cargo_build(workspace_root: &Path, release: bool) -> Result<bool> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("-p")
        .arg(ROOT_PACKAGE)
        .current_dir(workspace_root);

    if release {
        cmd.arg("--release");
    }

    let status = cmd.status().context("failed to run cargo build")?;
    Ok(status.success())
}

fn launch_app(bundle_path: &Path) -> Result<Arc<SharedChild>> {
    let binary = bundle_path.join("Contents/MacOS/carrot");
    log::info!("🚀 Launching {}", binary.display());

    let cmd = Command::new(&binary)
        .env("CARROT_BUNDLE_PATH", bundle_path)
        .spawn()
        .with_context(|| format!("failed to launch {}", binary.display()))?;

    Ok(Arc::new(
        SharedChild::new(cmd).expect("failed to wrap child process"),
    ))
}

fn kill_process_tree(child: &Arc<SharedChild>) {
    let pid = child.id();

    if let Ok(output) = Command::new("sh")
        .arg("-c")
        .arg(format!(
            r#"
            get_children() {{
                local cpids=$(pgrep -P "$1" 2>/dev/null)
                for cpid in $cpids; do
                    get_children "$cpid"
                    echo "$cpid"
                done
            }}
            get_children {}
            "#,
            pid
        ))
        .output()
    {
        let pids = String::from_utf8_lossy(&output.stdout);
        for line in pids.lines() {
            if let Ok(cpid) = line.trim().parse::<i32>() {
                unsafe {
                    libc::kill(cpid, libc::SIGKILL);
                }
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}
