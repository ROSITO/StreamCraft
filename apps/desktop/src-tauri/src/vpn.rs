//! System VPN indicator — ports Swift SystemVPNMonitor (scutil --nc list).

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelProvider {
    Tailscale,
    Nordvpn,
    Stormshield,
    Other,
}

impl TunnelProvider {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tailscale => "Tailscale",
            Self::Nordvpn => "NordVPN",
            Self::Stormshield => "Stormshield",
            Self::Other => "VPN",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedTunnel {
    pub interface_name: Option<String>,
    pub service_name: String,
    pub provider: TunnelProvider,
    pub ipv4_addresses: Vec<String>,
    pub is_connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemVpnStatus {
    pub is_active: bool,
    pub short_label: String,
    pub detail: String,
    pub tunnels: Vec<DetectedTunnel>,
    pub tailscale_connected: bool,
    pub nordvpn_connected: bool,
}

impl SystemVpnStatus {
    pub fn inactive() -> Self {
        Self {
            is_active: false,
            short_label: "VPN inactif".into(),
            detail: "Aucun VPN système connecté".into(),
            tunnels: vec![],
            tailscale_connected: false,
            nordvpn_connected: false,
        }
    }
}

pub fn refresh(filter: streamcraft_core::VpnIndicatorFilter) -> SystemVpnStatus {
    let tunnels = detect_tunnels();
    build_status(tunnels, filter)
}

fn detect_tunnels() -> Vec<DetectedTunnel> {
    let mut tunnels = parse_scutil_nc_list();
    // Tailscale CGNAT safety net via ifconfig-like: parse `ifconfig` lightly
    let has_ts = tunnels
        .iter()
        .any(|t| t.provider == TunnelProvider::Tailscale && t.is_connected);
    if !has_ts {
        for (iface, ips) in interface_ipv4_cgnat() {
            tunnels.push(DetectedTunnel {
                interface_name: Some(iface),
                service_name: "Tailscale (interface)".into(),
                provider: TunnelProvider::Tailscale,
                ipv4_addresses: ips,
                is_connected: true,
            });
        }
    }
    tunnels
}

fn parse_scutil_nc_list() -> Vec<DetectedTunnel> {
    let output = Command::new("/usr/sbin/scutil")
        .args(["--nc", "list"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    parse_scutil_output(&output)
}

pub fn parse_scutil_output(output: &str) -> Vec<DetectedTunnel> {
    let mut result = Vec::new();
    for line in output.lines() {
        if !line.contains("(Connected)") {
            continue;
        }
        let service_name = extract_quoted_name(line).unwrap_or_else(|| "VPN".into());
        let provider = classify_service(&service_name, line);
        result.push(DetectedTunnel {
            interface_name: None,
            service_name,
            provider,
            ipv4_addresses: vec![],
            is_connected: true,
        });
    }
    result
}

fn extract_quoted_name(line: &str) -> Option<String> {
    let start = line.find('"')?;
    let rest = &line[start + 1..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn classify_service(name: &str, line: &str) -> TunnelProvider {
    let hay = format!("{name} {line}").to_lowercase();
    if hay.contains("tailscale") {
        TunnelProvider::Tailscale
    } else if hay.contains("nordvpn") || hay.contains("nord vpn") || hay.contains("nordlynx") {
        TunnelProvider::Nordvpn
    } else if hay.contains("stormshield") || hay.contains("storm shield") {
        TunnelProvider::Stormshield
    } else {
        TunnelProvider::Other
    }
}

fn is_tailscale_cgnat(ip: &str) -> bool {
    let parts: Vec<_> = ip.split('.').filter_map(|p| p.parse::<u32>().ok()).collect();
    parts.len() == 4 && parts[0] == 100 && (64..=127).contains(&parts[1])
}

fn interface_ipv4_cgnat() -> Vec<(String, Vec<String>)> {
    let output = Command::new("/sbin/ifconfig")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut ips: Vec<String> = Vec::new();
    for line in output.lines() {
        if !line.starts_with('\t') && !line.starts_with(' ') && line.contains(':') {
            if let Some(name) = current.take() {
                if looks_like_vpn(&name) {
                    let cgnat: Vec<_> = ips.into_iter().filter(|ip| is_tailscale_cgnat(ip)).collect();
                    if !cgnat.is_empty() {
                        out.push((name, cgnat));
                    }
                }
            }
            current = Some(line.split(':').next().unwrap_or("").to_string());
            ips = Vec::new();
            continue;
        }
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            let ip = rest.split_whitespace().next().unwrap_or("").to_string();
            if !ip.is_empty() && !ip.starts_with("169.254.") {
                ips.push(ip);
            }
        }
    }
    if let Some(name) = current {
        if looks_like_vpn(&name) {
            let cgnat: Vec<_> = ips.into_iter().filter(|ip| is_tailscale_cgnat(ip)).collect();
            if !cgnat.is_empty() {
                out.push((name, cgnat));
            }
        }
    }
    out
}

fn looks_like_vpn(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.starts_with("utun")
        || lower.starts_with("ipsec")
        || lower.starts_with("ppp")
        || (lower.starts_with("tun") && !lower.starts_with("tunnel"))
        || lower.contains("nordlynx")
}

pub fn build_status(
    tunnels: Vec<DetectedTunnel>,
    filter: streamcraft_core::VpnIndicatorFilter,
) -> SystemVpnStatus {
    use streamcraft_core::VpnIndicatorFilter;
    let connected: Vec<_> = tunnels.iter().filter(|t| t.is_connected).cloned().collect();
    let tailscale_connected = connected.iter().any(|t| t.provider == TunnelProvider::Tailscale);
    let nordvpn_connected = connected.iter().any(|t| t.provider == TunnelProvider::Nordvpn);

    let relevant: Vec<_> = match filter {
        VpnIndicatorFilter::All => connected.clone(),
        VpnIndicatorFilter::IgnoreTailscale => connected
            .iter()
            .filter(|t| t.provider != TunnelProvider::Tailscale)
            .cloned()
            .collect(),
        VpnIndicatorFilter::NordVpnOnly => connected
            .iter()
            .filter(|t| t.provider == TunnelProvider::Nordvpn)
            .cloned()
            .collect(),
    };

    let is_active = !relevant.is_empty();
    let short_label = if !is_active {
        "VPN inactif".to_string()
    } else {
        let providers: std::collections::BTreeSet<_> = relevant.iter().map(|t| t.provider).collect();
        if providers.len() == 1 {
            let only = *providers.iter().next().unwrap();
            match only {
                TunnelProvider::Nordvpn => "NordVPN actif".into(),
                TunnelProvider::Tailscale => "Tailscale actif".into(),
                TunnelProvider::Stormshield => "Stormshield actif".into(),
                TunnelProvider::Other => "VPN actif".into(),
            }
        } else {
            "VPN actif".into()
        }
    };

    let mut detail_parts = Vec::new();
    if connected.is_empty() {
        detail_parts.push("Aucun service VPN en état Connected (scutil)".into());
    } else {
        for tunnel in &connected {
            let mut line = format!("{} : {}", tunnel.provider.label(), tunnel.service_name);
            if let Some(iface) = &tunnel.interface_name {
                line.push_str(&format!(" ({iface})"));
            }
            if !tunnel.ipv4_addresses.is_empty() {
                line.push_str(&format!(" · {}", tunnel.ipv4_addresses.join(", ")));
            }
            detail_parts.push(line);
        }
    }
    if tailscale_connected && matches!(filter, VpnIndicatorFilter::IgnoreTailscale) {
        detail_parts.push("Tailscale connecté (ignoré pour le voyant)".into());
    }
    if matches!(filter, VpnIndicatorFilter::NordVpnOnly) && !nordvpn_connected {
        detail_parts.push(
            "NordVPN non connecté — l’app peut tourner en arrière-plan sans tunnel actif".into(),
        );
    }

    SystemVpnStatus {
        is_active,
        short_label,
        detail: detail_parts.join(" · "),
        tunnels,
        tailscale_connected,
        nordvpn_connected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamcraft_core::VpnIndicatorFilter;

    #[test]
    fn parse_connected_nord() {
        let sample = r#"
* (Connected)      00000000-0000-0000-0000-000000000001 VPN (com.nordvpn...) "NordVPN" [VPN:com.nordvpn]
  (Disconnected)  00000000-0000-0000-0000-000000000002 VPN (io.tailscale...) "Tailscale" [VPN:io.tailscale]
"#;
        let tunnels = parse_scutil_output(sample);
        assert_eq!(tunnels.len(), 1);
        assert_eq!(tunnels[0].provider, TunnelProvider::Nordvpn);
        let status = build_status(tunnels, VpnIndicatorFilter::IgnoreTailscale);
        assert!(status.is_active);
        assert_eq!(status.short_label, "NordVPN actif");
    }
}
