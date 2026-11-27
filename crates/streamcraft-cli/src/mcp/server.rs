//! rmcp 3.5 MCP server — tools wrap the `streamcraft` CLI surface.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig, Implementation};
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::process::Command;

use super::roots::AutomationRoots;

#[derive(Clone)]
pub struct StreamcraftMcp {
    roots: Arc<AutomationRoots>,
    tool_router: ToolRouter<Self>,
}

impl StreamcraftMcp {
    pub fn new(roots: AutomationRoots) -> Self {
        Self {
            roots: Arc::new(roots),
            tool_router: Self::tool_router(),
        }
    }

    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        const MAX_MCP_LINE: usize = 64 * 1024 * 1024;
        let craft_jsonrpc::async_lines::Adapter {
            service_in,
            service_out,
            tasks,
        } = craft_jsonrpc::async_lines::spawn(
            tokio::io::stdin(),
            tokio::io::stdout(),
            MAX_MCP_LINE,
            MAX_MCP_LINE,
        );
        let running = self
            .serve((service_in, service_out))
            .await
            .map_err(|e| anyhow::anyhow!("MCP init: {e}"))?;
        let r = running
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("{e}"));
        tasks.finish().await;
        r.map(|_| ())
    }

    async fn run_cli(&self, argv: &[String]) -> Result<Value, McpError> {
        let exe = std::env::current_exe().map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let mut args = argv.to_vec();
        if !args.iter().any(|a| a == "--json") {
            args.push("--json".into());
        }
        let output = Command::new(&exe)
            .args(&args)
            .env("STREAMCRAFT_EXPORT_DIR", self.roots.write_root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let events: Vec<Value> = stdout
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        Ok(json!({
            "exit_code": output.status.code().unwrap_or(-1),
            "events": events,
            "stderr": stderr,
        }))
    }
}

/// Agent-supplied positionals must not be parsed as CLI flags (`--out=/elsewhere`).
fn positional(s: String) -> Result<String, McpError> {
    if s.starts_with('-') {
        return Err(McpError::invalid_params(format!("argument must not start with '-': {s}"), None));
    }
    Ok(s)
}

fn ok_json(v: Value) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(v.to_string())]))
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct EmptyParams {}

#[derive(Debug, Deserialize, JsonSchema)]
struct DownloadParams {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    out: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FetchParams {
    page_url: String,
    #[serde(default)]
    detect_only: Option<bool>,
    #[serde(default)]
    auto_download: Option<bool>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SeasonParams {
    show_or_url: String,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    list_only: Option<bool>,
    #[serde(default)]
    auto_download: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct FlixDigestParams {
    #[serde(default)]
    max: Option<u32>,
    #[serde(default)]
    mark_seen: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct FlixPickParams {
    index: u32,
    #[serde(default)]
    enqueue: Option<bool>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
struct StatusParams {
    #[serde(default)]
    job_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WaitParams {
    job_id: String,
    #[serde(default)]
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CancelParams {
    job_id: String,
}

#[tool_router]
impl StreamcraftMcp {
    #[tool(
        title = "Doctor",
        description = "Check FFmpeg sidecar resolution and automation roots",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn doctor(&self, Parameters(_p): Parameters<EmptyParams>) -> Result<CallToolResult, McpError> {
        let mut v = self.run_cli(&["doctor".into()]).await?;
        if let Some(obj) = v.as_object_mut() {
            obj.insert(
                "write_root".into(),
                json!(self.roots.write_root().display().to_string()),
            );
            obj.insert("read_roots".into(), json!(self.roots.read_roots()));
        }
        ok_json(v)
    }

    #[tool(
        title = "Download",
        description = "Download a clear media URL into the automation write root",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
    async fn download(&self, Parameters(p): Parameters<DownloadParams>) -> Result<CallToolResult, McpError> {
        let out = match p.out {
            Some(o) => self
                .roots
                .ensure_writable(PathBuf::from(o).as_path())
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?,
            None => self.roots.write_root().to_path_buf(),
        };
        let mut argv = vec!["download".into(), positional(p.url)?, "--out".into(), out.display().to_string()];
        if let Some(t) = p.title {
            argv.push("--title".into());
            argv.push(t);
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Fetch",
        description = "Detect media on a page (headless scrape)",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
    async fn fetch(&self, Parameters(p): Parameters<FetchParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["fetch".into(), positional(p.page_url)?];
        if p.detect_only.unwrap_or(false) {
            argv.push("--detect-only".into());
        }
        if p.auto_download.unwrap_or(false) {
            argv.push("--auto-download".into());
        }
        if let Some(t) = p.title {
            argv.push("--title".into());
            argv.push(t);
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Season",
        description = "List or download episodes for a show",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
    async fn season(&self, Parameters(p): Parameters<SeasonParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["season".into(), positional(p.show_or_url)?];
        if let Some(s) = p.season {
            argv.push("--season".into());
            argv.push(s.to_string());
        }
        let auto = p.auto_download.unwrap_or(false);
        let list_only = p.list_only.unwrap_or(true);
        if auto {
            argv.push("--auto-download".into());
        } else if list_only {
            argv.push("--list-only".into());
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Flix digest",
        description = "Digest RSS novelties from favourite hubs",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = true)
    )]
    async fn flix_digest(&self, Parameters(p): Parameters<FlixDigestParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["flix".into(), "digest".into()];
        if let Some(m) = p.max {
            argv.push("--max".into());
            argv.push(m.to_string());
        }
        if p.mark_seen.unwrap_or(false) {
            argv.push("--mark-seen".into());
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Flix pick",
        description = "Pick a digest entry; optional enqueue",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = true)
    )]
    async fn flix_pick(&self, Parameters(p): Parameters<FlixPickParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["flix".into(), "pick".into(), p.index.to_string()];
        if p.enqueue.unwrap_or(false) {
            argv.push("--enqueue".into());
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Status",
        description = "Job status",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn status(&self, Parameters(p): Parameters<StatusParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["status".into()];
        if let Some(id) = p.job_id {
            argv.push(positional(id)?);
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "List jobs",
        description = "List download jobs",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list(&self, Parameters(_p): Parameters<EmptyParams>) -> Result<CallToolResult, McpError> {
        ok_json(self.run_cli(&["list".into()]).await?)
    }

    #[tool(
        title = "Wait",
        description = "Wait for a job to finish",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    async fn wait(&self, Parameters(p): Parameters<WaitParams>) -> Result<CallToolResult, McpError> {
        let mut argv = vec!["wait".into(), positional(p.job_id)?];
        if let Some(t) = p.timeout {
            argv.push("--timeout".into());
            argv.push(t.to_string());
        }
        ok_json(self.run_cli(&argv).await?)
    }

    #[tool(
        title = "Cancel",
        description = "Cancel a job",
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn cancel(&self, Parameters(p): Parameters<CancelParams>) -> Result<CallToolResult, McpError> {
        ok_json(self.run_cli(&["cancel".into(), positional(p.job_id)?]).await?)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for StreamcraftMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("streamcraft", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "StreamCraft downloads clear HLS/DASH/MP4. Tools: doctor, download, fetch, season, flix_digest, flix_pick, status, list, wait, cancel. Paths must stay under --automation-write-root / --automation-read-root."
                    .to_string(),
            )
    }
}

