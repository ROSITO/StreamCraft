use serde::Serialize;

#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum ExitCode {
    Ok = 0,
    Usage = 2,
    NotFound = 3,
    Network = 4,
    Ffmpeg = 5,
    Cancelled = 6,
    Drm = 7,
    Internal = 10,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Log { level: String, message: String },
    Detected { url: String, kind: String, title: Option<String> },
    Progress { job_id: String, progress: f64, state: String },
    Completed { job_id: String, path: String },
    Failed { job_id: Option<String>, message: String },
    Status { jobs: Vec<serde_json::Value> },
    FlixDigest { count: usize, items: Vec<serde_json::Value> },
    SeasonList { show: String, episodes: Vec<serde_json::Value> },
}

impl AgentEvent {
    pub fn emit(&self) {
        if let Ok(line) = serde_json::to_string(self) {
            println!("{line}");
        }
    }
}
