//! Craft AI role catalogue for StreamCraft (craft-ai-core).

use craft_ai_core::capability::CapabilityId;
use craft_ai_core::ids::RoleId;
use craft_ai_core::role::Role;

/// Roles an agent / launcher can bind (automation + future AI assist).
pub fn streamcraft_roles() -> Vec<Role> {
    [
        ("streamcraft.download", "Download a clear media URL", "text.complete"),
        ("streamcraft.fetch", "Detect media on a page URL", "text.complete"),
        ("streamcraft.season", "List or download a season", "text.complete"),
        ("streamcraft.flix", "Digest hub RSS novelties", "text.complete"),
        ("streamcraft.status", "Inspect download jobs", "text.complete"),
        ("streamcraft.doctor", "Check FFmpeg sidecar", "text.complete"),
    ]
    .into_iter()
    .filter_map(|(id, label, cap)| {
        Some(Role {
            id: RoleId::new(id).ok()?,
            label: label.into(),
            capability: CapabilityId::parse(cap).ok()?,
            inputs: Vec::new(),
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_non_empty() {
        assert_eq!(streamcraft_roles().len(), 6);
    }
}
