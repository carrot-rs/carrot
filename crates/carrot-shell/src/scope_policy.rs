//! Classify a working directory into a `WorktreeRoot` — the path that the
//! workspace should anchor a worktree at. Runs on every shell cwd change, so
//! the walk is metadata-only (no recursion, no directory listing), bounded by
//! filesystem ancestors.

use std::path::{Path, PathBuf};

/// Where the worktree should be rooted for a given cwd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRoot {
    /// The cwd lives inside a project-marker directory (git repo, agent-rules
    /// file, or package manifest). `root` is the marker-bearing ancestor.
    ProjectLike {
        root: PathBuf,
        kind: ProjectKind,
        markers: ProjectMarkers,
    },
    /// No marker found in any ancestor. The cwd itself becomes the worktree root.
    AdHoc { cwd: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectKind {
    Git,
    AgentRules,
    Manifest(ManifestKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    CargoToml,
    PackageJson,
    PyprojectToml,
    GoMod,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectMarkers {
    pub git_root: Option<PathBuf>,
    pub agent_rules: Option<PathBuf>,
    pub manifest: Option<(PathBuf, ManifestKind)>,
}

/// Classify `cwd` by walking its ancestors and looking for project markers.
/// Priority: git-root > agent-rules > manifest > adhoc.
pub fn classify(cwd: &Path) -> WorktreeRoot {
    let markers = walk_up_collect_markers(cwd);

    if let Some(ref root) = markers.git_root {
        return WorktreeRoot::ProjectLike {
            root: root.clone(),
            kind: ProjectKind::Git,
            markers,
        };
    }

    if let Some(root) = markers
        .agent_rules
        .as_ref()
        .and_then(|p| p.parent().map(PathBuf::from))
    {
        return WorktreeRoot::ProjectLike {
            root,
            kind: ProjectKind::AgentRules,
            markers,
        };
    }

    if let Some((path, kind)) = markers.manifest {
        let root = path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf());
        return WorktreeRoot::ProjectLike {
            root,
            kind: ProjectKind::Manifest(kind),
            markers: ProjectMarkers {
                manifest: Some((path, kind)),
                ..Default::default()
            },
        };
    }

    WorktreeRoot::AdHoc {
        cwd: cwd.to_path_buf(),
    }
}

fn walk_up_collect_markers(cwd: &Path) -> ProjectMarkers {
    let mut markers = ProjectMarkers::default();

    for ancestor in cwd.ancestors() {
        if markers.git_root.is_none() && ancestor.join(".git").symlink_metadata().is_ok() {
            markers.git_root = Some(ancestor.to_path_buf());
        }

        if markers.agent_rules.is_none() {
            for name in ["AGENTS.md", "CLAUDE.md", "WARP.md"] {
                let candidate = ancestor.join(name);
                if candidate.symlink_metadata().is_ok() {
                    markers.agent_rules = Some(candidate);
                    break;
                }
            }
        }

        if markers.manifest.is_none() {
            for (name, kind) in [
                ("Cargo.toml", ManifestKind::CargoToml),
                ("package.json", ManifestKind::PackageJson),
                ("pyproject.toml", ManifestKind::PyprojectToml),
                ("go.mod", ManifestKind::GoMod),
            ] {
                let candidate = ancestor.join(name);
                if candidate.symlink_metadata().is_ok() {
                    markers.manifest = Some((candidate, kind));
                    break;
                }
            }
        }

        if markers.git_root.is_some()
            && markers.agent_rules.is_some()
            && markers.manifest.is_some()
        {
            break;
        }
    }

    markers
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn adhoc_when_no_markers() {
        let dir = tempdir();
        let result = classify(dir.path());
        match result {
            WorktreeRoot::AdHoc { cwd } => {
                assert_eq!(cwd, dir.path());
            }
            _ => panic!("expected AdHoc, got {result:?}"),
        }
    }

    #[test]
    fn git_root_takes_precedence_over_manifest() {
        let dir = tempdir();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join("Cargo.toml"), "[package]").unwrap();

        let result = classify(root);
        match result {
            WorktreeRoot::ProjectLike {
                root: r,
                kind: ProjectKind::Git,
                ..
            } => {
                assert_eq!(r, root);
            }
            _ => panic!("expected ProjectLike(Git), got {result:?}"),
        }
    }

    #[test]
    fn manifest_detected_without_git() {
        let dir = tempdir();
        let root = dir.path();
        fs::write(root.join("package.json"), "{}").unwrap();

        let result = classify(root);
        match result {
            WorktreeRoot::ProjectLike {
                root: r,
                kind: ProjectKind::Manifest(ManifestKind::PackageJson),
                ..
            } => {
                assert_eq!(r, root);
            }
            _ => panic!("expected Manifest(PackageJson), got {result:?}"),
        }
    }

    #[test]
    fn agent_rules_without_git_or_manifest() {
        let dir = tempdir();
        let root = dir.path();
        fs::write(root.join("AGENTS.md"), "").unwrap();

        let result = classify(root);
        match result {
            WorktreeRoot::ProjectLike {
                root: r,
                kind: ProjectKind::AgentRules,
                ..
            } => {
                assert_eq!(r, root);
            }
            _ => panic!("expected AgentRules, got {result:?}"),
        }
    }

    #[test]
    fn ancestor_walk_finds_git_root_from_subdir() {
        let dir = tempdir();
        let root = dir.path();
        fs::create_dir(root.join(".git")).unwrap();
        let sub = root.join("src/bin");
        fs::create_dir_all(&sub).unwrap();

        let result = classify(&sub);
        match result {
            WorktreeRoot::ProjectLike {
                root: r,
                kind: ProjectKind::Git,
                ..
            } => {
                assert_eq!(r, root);
            }
            _ => panic!("expected ProjectLike(Git), got {result:?}"),
        }
    }
}
