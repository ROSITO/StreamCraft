//! Local command bridge GUI ↔ CLI (`--via-app`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Error)]
pub enum ViaAppError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("via-app server not reachable on {0}")]
    NotReachable(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ViaAppRequest {
    Fetch { url: String },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ViaAppResponse {
    Pong,
    Html { url: String, html: String, title: Option<String> },
    Error { message: String },
}

pub fn default_addr() -> String {
    std::env::var("OPENSTREAM_VIA_APP")
        .unwrap_or_else(|_| "127.0.0.1:8787".into())
}

pub fn inbox_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("StreamCraft")
        .join("inbox")
}

/// CLI client: send one JSON line request, read one JSON line response.
pub async fn request(req: ViaAppRequest) -> Result<ViaAppResponse, ViaAppError> {
    let addr = default_addr();
    let stream = TcpStream::connect(&addr)
        .await
        .map_err(|_| ViaAppError::NotReachable(addr.clone()))?;
    let (reader, mut writer) = stream.into_split();
    let line = serde_json::to_string(&req)? + "\n";
    writer.write_all(line.as_bytes()).await?;
    let mut lines = BufReader::new(reader).lines();
    let Some(resp_line) = lines.next_line().await? else {
        return Ok(ViaAppResponse::Error {
            message: "empty response".into(),
        });
    };
    Ok(serde_json::from_str(&resp_line)?)
}

/// GUI host: accept connections and answer Ping; Fetch is handled by callback returning HTML.
pub async fn serve_ping_only() -> Result<(), ViaAppError> {
    let addr = default_addr();
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!(%addr, "via-app listening");
    loop {
        let (socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(e) = handle_conn(socket).await {
                tracing::warn!(error = %e, "via-app connection error");
            }
        });
    }
}

async fn handle_conn(stream: TcpStream) -> Result<(), ViaAppError> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        let req: ViaAppRequest = serde_json::from_str(&line)?;
        let resp = match req {
            ViaAppRequest::Ping => ViaAppResponse::Pong,
            ViaAppRequest::Fetch { url } => ViaAppResponse::Error {
                message: format!(
                    "Fetch via GUI WebView not wired for {url}; use scrape CLI or open the page in-app"
                ),
            },
        };
        let out = serde_json::to_string(&resp)? + "\n";
        writer.write_all(out.as_bytes()).await?;
    }
    Ok(())
}
