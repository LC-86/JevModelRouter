//! Static Grok helper source/version gate.
//!
//! This module does not launch the CLI, initialize ACP, read local auth state, or fetch account,
//! catalog, billing, or auto-top-up data. Its Unknown fields describe this legacy static snapshot;
//! the separate manual observation in [`super::readonly`] does not change source provenance or
//! generation admission.

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UiStatus {
    pub state: &'static str,
    pub source_verified: bool,
    pub source_version: Option<&'static str>,
    pub source_commit: Option<&'static str>,
    pub auth_info: &'static str,
    pub model_catalog: &'static str,
    pub billing: &'static str,
    pub auto_topup_rule: &'static str,
    pub extra_usage_permission: &'static str,
    pub real_generation_enabled: bool,
}

/// The installed CLI and its public source are not connected to this app build.
pub(crate) fn ui_status() -> UiStatus {
    UiStatus {
        state: "blocked_unverified_source",
        source_verified: false,
        source_version: None,
        source_commit: None,
        auth_info: "unknown",
        model_catalog: "unknown",
        billing: "unknown",
        auto_topup_rule: "unknown",
        extra_usage_permission: "unknown",
        real_generation_enabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::ui_status;

    #[test]
    fn production_status_keeps_unverified_source_and_all_grok_data_unknown() {
        let status = ui_status();
        assert_eq!(status.state, "blocked_unverified_source");
        assert!(!status.source_verified);
        assert!(status.source_version.is_none());
        assert!(status.source_commit.is_none());
        assert_eq!(status.auth_info, "unknown");
        assert_eq!(status.model_catalog, "unknown");
        assert_eq!(status.billing, "unknown");
        assert_eq!(status.auto_topup_rule, "unknown");
        assert_eq!(status.extra_usage_permission, "unknown");
        assert!(!status.real_generation_enabled);
    }
}
