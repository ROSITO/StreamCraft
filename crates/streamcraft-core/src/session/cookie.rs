//! CookieBridge snapshot — port Swift `SessionCredentialSnapshot`.
//!
//! Origin: Tauri `cookie_refresh` + `start_download` (removed, see commit `e68165d`).

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CookieEntry {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionCredentialSnapshot {
    pub cookies: Vec<CookieEntry>,
}

impl SessionCredentialSnapshot {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_document_cookie(raw: &str, host: &str) -> Self {
        let cookies = raw
            .split(';')
            .filter_map(|pair| {
                let pair = pair.trim();
                if pair.is_empty() {
                    return None;
                }
                let (name, value) = pair.split_once('=')?;
                Some(CookieEntry {
                    name: name.trim().to_string(),
                    value: value.trim().to_string(),
                    domain: host.to_string(),
                    path: "/".into(),
                })
            })
            .collect();
        Self { cookies }
    }

    pub fn cookie_header_for(&self, url: &Url) -> Option<String> {
        let relevant: Vec<&CookieEntry> = self
            .cookies
            .iter()
            .filter(|c| cookie_matches(c, url))
            .collect();
        if relevant.is_empty() {
            return None;
        }
        Some(
            relevant
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

fn cookie_matches(cookie: &CookieEntry, url: &Url) -> bool {
    let Some(host) = url.host_str().map(|h| h.to_lowercase()) else {
        return false;
    };
    let domain = cookie
        .domain
        .to_lowercase()
        .trim_start_matches('.')
        .to_string();
    // Dot boundary required: `example.com` must not match `evilexample.com`.
    let host_ok = host == domain || host.ends_with(&format!(".{domain}"));
    if !host_ok {
        return false;
    }
    let path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    path.starts_with(&cookie.path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_subdomain() {
        let snap = SessionCredentialSnapshot {
            cookies: vec![CookieEntry {
                name: "sid".into(),
                value: "1".into(),
                domain: ".example.com".into(),
                path: "/".into(),
            }],
        };
        let u = Url::parse("https://cdn.example.com/a.ts").unwrap();
        assert_eq!(snap.cookie_header_for(&u).as_deref(), Some("sid=1"));
        let evil = Url::parse("https://evilexample.com/a.ts").unwrap();
        assert_eq!(snap.cookie_header_for(&evil), None);
    }
}
