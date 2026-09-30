use crate::config::AppConfig;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry { pub binding: String, pub id: String, pub name: String }
pub fn supported(agent: &str) -> bool { matches!(agent, "hermes" | "opencode" | "openclaw" | "omp" | "kimi" | "grok") }
pub fn reconnect_targets(config: &AppConfig) -> Vec<(&str, &str)> {
    let mut targets: Vec<_> = config.agent_selections.iter()
        .filter(|(id, _)| config.agent_auto_connect.get(*id).copied().unwrap_or(true))
        .map(|(id, binding)| (id.as_str(), binding.as_str())).collect();
    targets.sort_unstable_by_key(|(id, _)| *id);
    targets
}
pub fn build(config: &AppConfig, bindings: &[String]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for binding in bindings {
        if entries.iter().any(|e: &Entry| &e.binding == binding) { continue; }
        let (id, name) = if let Some(id) = binding.strip_prefix("model/") {
            // 已选、未停用且服务商启用才进入 Agent 可选列表；取消选择与停用都让该绑定不可用。
            let m = config.models.iter().find(|m| m.id == id && m.selected && m.enabled && config.providers.iter().any(|p| p.id == m.provider_id && p.enabled)).ok_or_else(|| anyhow!("Model is unavailable"))?;
            (format!("{}/{}", m.provider_id, m.model_id), m.name.clone())
        } else {
            let r = config.routes.iter().find(|r| &r.id == binding && r.enabled).ok_or_else(|| anyhow!("Route is unavailable"))?;
            crate::router::validate_available_rule(config, r)?;
            (format!("autojev/{}", r.id), r.name.clone())
        };
        if entries.iter().any(|e: &Entry| e.id == id) { return Err(anyhow!("Selected models have conflicting public IDs")); }
        entries.push(Entry { binding: binding.clone(), id, name });
    }
    if entries.is_empty() { return Err(anyhow!("Select at least one model or route")); }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconnect_restores_saved_connections_but_respects_manual_disconnect() {
        let mut config = AppConfig::default();
        config.agent_selections.insert("codex".into(), "cheap".into());
        config.agent_selections.insert("hermes".into(), "fast".into());
        assert_eq!(reconnect_targets(&config), vec![("codex", "cheap"), ("hermes", "fast")]);
        config.agent_auto_connect.insert("codex".into(), false);
        let restored: AppConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(reconnect_targets(&restored), vec![("hermes", "fast")]);
        config.agent_auto_connect.insert("codex".into(), true);
        assert_eq!(reconnect_targets(&config).len(), 2);
    }
    #[test]
    fn unselected_or_disabled_models_leave_the_agent_selectable_list() {
        let mut config = AppConfig::default();
        let binding = format!("model/{}", config.models[0].id);
        assert!(build(&config, &[binding.clone()]).is_ok());
        // 取消选择：移出 Agent 可选列表（列表是注入清单，不承担直调能力）。
        config.models[0].selected = false;
        assert!(build(&config, &[binding.clone()]).unwrap_err().to_string().contains("unavailable"));
        config.models[0].selected = true;
        // 停用：同样不可选。
        config.models[0].enabled = false;
        assert!(build(&config, &[binding.clone()]).is_err());
        config.models[0].enabled = true;
        // 服务商停用：不可选。
        config.providers[0].enabled = false;
        assert!(build(&config, &[binding]).is_err());
    }

    #[test]
    fn routes_with_deleted_candidates_can_connect_but_empty_routes_cannot() {
        let mut config = AppConfig::default();
        let id = config.models[0].id.clone();
        let route = crate::config::RouteRule {
            id: "legacy".into(), name: "Legacy".into(), strategy: "round_robin".into(),
            all_models: false, automatic_policy: None, model_settings: Default::default(),
            model_ids: vec!["deleted-model".into(), id.clone(), id.clone()], enabled: true,
        };
        config.routes = vec![route];
        let bindings = vec!["legacy".into(), format!("model/{id}")];
        let catalog = build(&config, &bindings).unwrap();
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].binding, "legacy");
        assert_eq!(catalog[0].id, "autojev/legacy");
        assert_eq!(catalog[1].id, format!("{}/{}", config.models[0].provider_id, config.models[0].model_id));
        for entry in &catalog {
            assert_eq!(crate::router::normalize_requested_model(&config, Some(&entry.id)).unwrap(), Some(format!("autojev/{}",entry.binding)));
        }
        assert_eq!(crate::router::normalize_requested_model(&config, Some("route/legacy")).unwrap(), Some("autojev/legacy".into()));
        assert_eq!(crate::router::normalize_requested_model(&config, Some(&format!("model/{}",config.models[0].model_id))).unwrap(), Some(format!("autojev/model/{id}")));
        assert!(crate::router::normalize_requested_model(&config, Some("model/not-real")).is_err());
        assert!(crate::router::normalize_requested_model(&config, Some("route/not-real")).is_err());
        // Validation must not mutate or expand the user's candidate scope.
        assert_eq!(config.routes[0].model_ids.len(), 3);
        config.models[0].enabled = false;
        assert!(build(&config, &["legacy".into()]).is_err());
        config.models[0].enabled = true;
        config.routes[0].model_ids = vec!["deleted-model".into()];
        assert!(build(&config, &["legacy".into()]).is_err());
    }
    #[test]
    fn qualified_ids_preserve_nested_model_names_and_take_precedence_over_bare_names() {
        let mut config = AppConfig::default();
        config.models[0].model_id = "vendor/model".into();
        let binding = format!("model/{}", config.models[0].id);
        let public_id = format!("{}/vendor/model", config.models[0].provider_id);
        let mut other = config.models[0].clone();
        other.id = "other-model".into();
        other.model_id = public_id.clone();
        config.models.push(other);
        let catalog = build(&config, &[binding.clone()]).unwrap();
        assert_eq!(catalog[0].id, public_id);
        assert_eq!(crate::router::normalize_requested_model(&config, Some(&public_id)).unwrap(), Some(format!("autojev/{binding}")));
    }
    #[test]
    fn duplicate_upstream_names_get_distinct_ids_and_disabled_models_are_rejected() {
        let mut config = AppConfig::default();
        let mut second = config.models[0].clone();
        second.id = "second".into();
        let mut provider = config.providers[0].clone();
        provider.id = "another".into();
        second.provider_id = provider.id.clone();
        config.providers.push(provider);
        config.models.push(second);
        let bindings = vec![format!("model/{}", config.models[0].id), "model/second".into()];
        let catalog = build(&config, &bindings).unwrap();
        assert_ne!(catalog[0].id, catalog[1].id);
        assert_eq!(catalog[0].id,format!("{}/{}",config.models[0].provider_id,config.models[0].model_id));
        for entry in &catalog {
            assert_eq!(crate::router::normalize_requested_model(&config, Some(&entry.id)).unwrap(), Some(format!("autojev/{}",entry.binding)));
        }
        config.models.last_mut().unwrap().enabled = false;
        assert!(build(&config, &bindings).is_err());
    }
}

// Injection callers provide a public model ID or an autojev/<route> ID.
pub fn wire_id(public_id: &str) -> String { public_id.into() }
