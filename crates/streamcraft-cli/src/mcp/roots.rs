//! Fail-closed filesystem roots (PhotoCraft-style, simplified).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use cap_std::ambient_authority;
use cap_std::fs::Dir;

#[derive(Clone)]
struct RootCap {
    dir: Arc<Dir>,
    configured: PathBuf,
    canonical: PathBuf,
}

#[derive(Clone)]
pub struct AutomationRoots {
    read: Vec<RootCap>,
    write: RootCap,
}

impl AutomationRoots {
    /// Open every configured root at launch. Missing/unreadable roots fail closed.
    pub fn open(read_roots: Vec<PathBuf>, write_root: PathBuf) -> Result<Self> {
        let mut read = Vec::with_capacity(read_roots.len());
        for path in &read_roots {
            read.push(open_root(path).with_context(|| format!("read root {}", path.display()))?);
        }
        let write = open_root(&write_root)
            .with_context(|| format!("write root {}", write_root.display()))?;
        Ok(Self { read, write })
    }

    pub fn write_root(&self) -> &Path {
        &self.write.configured
    }

    pub fn read_roots(&self) -> Vec<PathBuf> {
        self.read.iter().map(|r| r.configured.clone()).collect()
    }

    /// Absolute path must sit under the write root (canonical or configured spelling).
    pub fn ensure_writable(&self, path: &Path) -> Result<PathBuf> {
        under_root(path, &self.write)
    }

    /// Absolute path must sit under a read root or the write root.
    #[allow(dead_code)]
    pub fn ensure_readable(&self, path: &Path) -> Result<PathBuf> {
        for root in self.read.iter().chain(std::iter::once(&self.write)) {
            if let Ok(p) = under_root(path, root) {
                return Ok(p);
            }
        }
        bail!(
            "automation filesystem access is not granted: path outside roots: {}",
            path.display()
        )
    }
}

fn open_root(path: &Path) -> Result<RootCap> {
    if !path.is_absolute() {
        bail!("root must be absolute: {}", path.display());
    }
    std::fs::create_dir_all(path)
        .with_context(|| format!("create root {}", path.display()))?;
    let dir = Dir::open_ambient_dir(path, ambient_authority())
        .with_context(|| format!("open capability for {}", path.display()))?;
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("canonicalize {}", path.display()))?;
    Ok(RootCap {
        dir: Arc::new(dir),
        configured: path.to_path_buf(),
        canonical,
    })
}

fn under_root(path: &Path, root: &RootCap) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("path must be absolute: {}", path.display());
    }
    let candidate = if path.exists() {
        std::fs::canonicalize(path)?
    } else if let Some(parent) = path.parent() {
        let parent_canon = if parent.exists() {
            std::fs::canonicalize(parent)?
        } else if parent.starts_with(&root.canonical) || parent.starts_with(&root.configured) {
            parent.to_path_buf()
        } else {
            bail!(
                "automation filesystem access is not granted: {}",
                path.display()
            );
        };
        let name = path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("invalid path {}", path.display()))?;
        parent_canon.join(name)
    } else {
        path.to_path_buf()
    };

    if candidate.starts_with(&root.canonical) || candidate.starts_with(&root.configured) {
        // Touch the capability so a dropped/closed root cannot be used later.
        let _ = root.dir.entries().context("capability still held")?;
        return Ok(candidate);
    }
    bail!(
        "automation filesystem access is not granted: {}",
        path.display()
    )
}
