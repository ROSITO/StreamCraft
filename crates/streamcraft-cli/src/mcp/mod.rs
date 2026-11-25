//! MCP stdio server for agents (D021). rmcp 3.5 + cap-std roots (Craft-aligned).

mod roots;
mod server;

use std::path::PathBuf;

use anyhow::{bail, Result};

pub use roots::AutomationRoots;
pub use server::StreamcraftMcp;

#[derive(Debug, Clone)]
pub struct McpConfig {
    pub read_roots: Vec<PathBuf>,
    pub write_root: PathBuf,
}

impl McpConfig {
    pub fn from_flags(read_roots: Vec<PathBuf>, write_root: Option<PathBuf>) -> Result<Self> {
        if read_roots.is_empty() {
            bail!("mcp requires at least one --automation-read-root");
        }
        let Some(write_root) = write_root else {
            bail!("mcp requires --automation-write-root");
        };
        if !write_root.is_absolute() {
            bail!("--automation-write-root must be absolute");
        }
        for r in &read_roots {
            if !r.is_absolute() {
                bail!("--automation-read-root must be absolute: {}", r.display());
            }
        }
        Ok(Self {
            read_roots,
            write_root,
        })
    }
}

pub async fn serve_stdio(config: McpConfig) -> Result<()> {
    let roots = AutomationRoots::open(config.read_roots, config.write_root)?;
    let server = StreamcraftMcp::new(roots);
    server.serve_stdio().await
}
