//! 订阅目录：上游发现的模型、账号绑定资格，以及「选择／停用」与「发现／资格」的分离。
//!
//! 三种状态各司其职，都不由同一个布尔值承担：
//!
//! - `Model.selected` / `Model.enabled`：用户配置。选择决定模型列表与自动候选，停用禁止所有调用；
//!   退出、换号、目录失败都不重建标识、也不丢配置。
//! - [`CatalogEntry::availability`]：上游与账号绑定的资格。随权威读取、读取失败、换号与撤销更新。
//! - `subscription::Evidence`：一次只读读取的原始证据，整体绑定连接世代。
//!
//! 发现是身份来源，选择是用户决定。新发现的模型默认未选；取消选择不禁止原模型标识的合规直调；
//! 只有停用（`Model.enabled = false`）或资格不可用才禁止调用。

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, Model, ModelTier};
use crate::subscription::DiscoveredModel;

/// 目录项在当前账号与连接世代下的可用性。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// 当前账号与世代下的权威目录确认可用。
    Available,
    /// 同一账号同一世代的读取失败：保留上次已核实结果，不把网络失败误判为模型被移除。
    Stale,
    /// 权威目录中已不存在：保留配置与选择，标不可用。
    Removed,
    /// 上游列出了该模型但当前账号没有使用权限：保留配置，标不可用。
    Revoked,
    /// 尚无当前账号与世代的资格依据（含退出、换号后的重置）。
    #[default]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// 上游限定模型 ID：调用身份的稳定来源。上游 ID 变化按新模型处理，不按相似显示名重绑。
    pub model_id: String,
    /// 上游显示名：只用于展示，改名不改变调用目标。
    #[serde(default)]
    pub name: Option<String>,
    /// 稳定内部标识，等于对应 `Model.id`；首次发现时分配，此后不重建。
    pub internal_id: String,
    #[serde(default)]
    pub availability: Availability,
    #[serde(default)]
    pub first_seen: Option<String>,
    #[serde(default)]
    pub last_confirmed: Option<String>,
    /// 资格最近一次被核实的连接世代与账号；与当前连接不一致即不构成资格。
    #[serde(default)]
    pub confirmed_generation: Option<u64>,
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCatalog {
    #[serde(default)]
    pub entries: Vec<CatalogEntry>,
}

impl ProviderCatalog {
    pub fn entry(&self, model_id: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| entry.model_id == model_id)
    }

    pub fn entry_mut(&mut self, model_id: &str) -> Option<&mut CatalogEntry> {
        self.entries.iter_mut().find(|entry| entry.model_id == model_id)
    }
}

/// 一次权威目录核对的结果：新增、移除与仍在目录中的模型（按上游 ID）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncOutcome {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub retained: Vec<String>,
}

/// 某个模型在当前账号与连接世代下的资格。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Eligibility {
    /// 具备调用资格；真正派发仍需能力与额度依据。
    Eligible,
    /// 同一账号同一世代的权威读取失败：沿用上次已核实资格，界面标陈旧。
    Stale,
    /// 当前账号的目录里没有这个模型。
    NotDiscovered,
    /// 权威目录确认已移除。
    Removed,
    /// 上游确认当前账号没有该模型权限。
    Revoked,
    /// 资格未绑定当前账号或连接世代（含退出、换号后尚未重核）。
    AccountChanged,
    /// 尚无资格依据。
    Unknown,
}

impl Eligibility {
    /// 陈旧但同账号同世代的已核实资格仍然成立：网络失败不等于模型被移除。
    pub fn is_eligible(self) -> bool {
        matches!(self, Eligibility::Eligible | Eligibility::Stale)
    }

    /// 准入拒绝的稳定机器码。`Eligible` / `Stale` 不产生拒绝。
    pub fn code(self) -> Option<&'static str> {
        match self {
            Eligibility::Eligible | Eligibility::Stale => None,
            Eligibility::NotDiscovered => Some("model_not_discovered"),
            Eligibility::Removed => Some("model_removed"),
            Eligibility::Revoked => Some("model_revoked"),
            Eligibility::AccountChanged => Some("model_unqualified"),
            Eligibility::Unknown => Some("model_unqualified"),
        }
    }

    /// 给界面与错误使用的单行原因。
    pub fn reason(self) -> &'static str {
        match self {
            Eligibility::Eligible => "the current account can use this model",
            Eligibility::Stale => "the directory read failed; the last confirmed qualification is kept",
            Eligibility::NotDiscovered => "the current account directory does not list this model",
            Eligibility::Removed => "the upstream directory no longer lists this model",
            Eligibility::Revoked => "the upstream account is not permitted to use this model",
            Eligibility::AccountChanged => "qualification has not been re-verified for the current account and connection",
            Eligibility::Unknown => "no directory qualification has been read for this connection",
        }
    }
}

pub fn catalog<'a>(config: &'a AppConfig, provider_id: &str) -> Option<&'a ProviderCatalog> {
    config.subscription_catalogs.get(provider_id)
}

/// 该服务商下已经建档的模型行（含已取消选择、已停用、已移除的项）。
pub fn models<'a>(config: &'a AppConfig, provider_id: &str) -> Vec<&'a Model> {
    config.models.iter().filter(|model| model.provider_id == provider_id).collect()
}

/// 进入模型列表与自动候选的行：既已选也已启用。取消选择只移出这里，不移除调用能力。
/// 这是「模型列表成员」的语义单点：生产路径经 `subscription::catalog_listed` 表达同一规则
/// （后者另加服务商启用与账号资格），测试与后续候选集合直接按它断言。
#[allow(dead_code)]
pub fn selected_models<'a>(config: &'a AppConfig, provider_id: &str) -> Vec<&'a Model> {
    models(config, provider_id).into_iter().filter(|model| model.selected && model.enabled).collect()
}

/// 权威目录读取成功后的核对。以本次结果整体替换该账号的目录：
/// 新增模型建档且默认未选，上游 ID 变化按新模型处理，本次结果中不存在的已核实项标 `Removed`。
/// 用户的选择与停用状态不因核对改变。
pub fn reconcile(
    config: &mut AppConfig,
    provider_id: &str,
    account: &str,
    generation: u64,
    discovered: &[DiscoveredModel],
    observed_at: Option<&str>,
) -> SyncOutcome {
    let account = account.trim();
    if account.is_empty() {
        return SyncOutcome::default();
    }
    let mut outcome = SyncOutcome::default();
    let entries: Vec<(String, Option<String>, Availability, String)> = discovered
        .iter()
        .map(|model| {
            (
                model.model_id.trim().to_owned(),
                model.name.clone(),
                if model.eligible { Availability::Available } else { Availability::Revoked },
                model.model_id.clone(),
            )
        })
        .filter(|(model_id, ..)| !model_id.is_empty())
        .collect();
    for (model_id, name, availability, raw_name) in &entries {
        let registered = config
            .subscription_catalogs
            .get(provider_id)
            .and_then(|catalog| catalog.entry(model_id))
            .map(|entry| entry.internal_id.clone());
        // 已经存在的模型行（旧配置或手工添加的订阅模型）是标识的权威来源：目录条目的内部标识
        // 必须等于对应 `Model.id`，不能另分配一个指向不存在模型行的新标识。
        let row_id = config
            .models
            .iter()
            .find(|model| model.provider_id == provider_id && model.model_id == *model_id)
            .map(|model| model.id.clone());
        match registered {
            None => {
                let internal_id = row_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                // 首次核对即为该服务商建立目录，不能因为目录原本不存在而丢掉建档结果。
                config.subscription_catalogs.entry(provider_id.to_owned()).or_default().entries.push(CatalogEntry {
                    model_id: model_id.clone(),
                    name: name.clone(),
                    internal_id: internal_id.clone(),
                    availability: *availability,
                    first_seen: observed_at.map(str::to_owned),
                    last_confirmed: observed_at.map(str::to_owned),
                    confirmed_generation: Some(generation),
                    account: Some(account.to_owned()),
                });
                outcome.added.push(model_id.clone());
                materialize(config, provider_id, model_id, name.as_deref().unwrap_or(raw_name), &internal_id);
            }
            Some(registered_id) => {
                // 模型行存在时以它为准，顺带修正早前版本可能留下的标识不一致。
                let internal_id = row_id.unwrap_or(registered_id);
                if let Some(entry) = config
                    .subscription_catalogs
                    .get_mut(provider_id)
                    .and_then(|catalog| catalog.entry_mut(model_id))
                {
                    if let Some(display) = name {
                        entry.name = Some(display.clone());
                    }
                    entry.internal_id = internal_id.clone();
                    entry.availability = *availability;
                    entry.last_confirmed = observed_at.map(str::to_owned);
                    entry.confirmed_generation = Some(generation);
                    entry.account = Some(account.to_owned());
                }
                outcome.retained.push(model_id.clone());
                if !config
                    .models
                    .iter()
                    .any(|model| model.provider_id == provider_id && model.model_id == *model_id)
                {
                    materialize(config, provider_id, model_id, name.as_deref().unwrap_or(raw_name), &internal_id);
                }
            }
        }
    }
    let known: Vec<String> = config
        .subscription_catalogs
        .get(provider_id)
        .map(|catalog| catalog.entries.iter().map(|entry| entry.model_id.clone()).collect())
        .unwrap_or_default();
    for model_id in known {
        if entries.iter().any(|(candidate, ..)| *candidate == model_id) {
            continue;
        }
        if let Some(entry) = config
            .subscription_catalogs
            .get_mut(provider_id)
            .and_then(|catalog| catalog.entry_mut(&model_id))
        {
            if matches!(entry.availability, Availability::Available | Availability::Stale | Availability::Unknown) {
                entry.availability = Availability::Removed;
                entry.confirmed_generation = Some(generation);
                entry.account = Some(account.to_owned());
                outcome.removed.push(model_id);
            }
        }
    }
    outcome
}

/// 首次发现即为模型建档：新模型默认未选、未停用，标识在退出与换号后保持稳定。
fn materialize(config: &mut AppConfig, provider_id: &str, model_id: &str, name: &str, internal_id: &str) {
    if config.models.iter().any(|model| model.provider_id == provider_id && model.model_id == model_id) {
        return;
    }
    config.models.push(Model {
        input_price_known: None,
        output_price_known: None,
        cache_price_known: None,
        api_type: String::new(),
        cache_cost_per_million: 0.0,
        id: internal_id.to_owned(),
        provider_id: provider_id.to_owned(),
        model_id: model_id.to_owned(),
        name: name.to_owned(),
        tier: ModelTier::Balanced,
        enabled: true,
        selected: false,
        supports_tools: false,
        supports_vision: false,
        supports_reasoning: false,
        context_window: 0,
        input_cost_per_million: 0.0,
        output_cost_per_million: 0.0,
    });
}

/// 同一账号的目录读取失败：保留已核实项，只标陈旧，不删除、不改选择与停用。
pub fn mark_stale(config: &mut AppConfig, provider_id: &str) {
    let Some(catalog) = config.subscription_catalogs.get_mut(provider_id) else { return };
    for entry in catalog.entries.iter_mut().filter(|entry| entry.availability == Availability::Available) {
        entry.availability = Availability::Stale;
    }
}

/// 退出或换号：账号相关资格整体失效，但保留用户的选择、停用与稳定标识。
/// 重核后按同一上游 ID 恢复资格，不沿用旧账号的目录证据。
pub fn invalidate_account(config: &mut AppConfig, provider_id: &str) {
    let Some(catalog) = config.subscription_catalogs.get_mut(provider_id) else { return };
    for entry in catalog.entries.iter_mut() {
        entry.availability = Availability::Unknown;
        entry.last_confirmed = None;
        entry.confirmed_generation = None;
        entry.account = None;
    }
}

/// 删除服务商时一并丢弃其目录。
pub fn forget_provider(config: &mut AppConfig, provider_id: &str) {
    config.subscription_catalogs.remove(provider_id);
}

/// 服务商标识重命名时迁移目录，保留标识、选择与停用。
pub fn rename_provider(config: &mut AppConfig, old_id: &str, new_id: &str) {
    if old_id == new_id {
        return;
    }
    if let Some(catalog) = config.subscription_catalogs.remove(old_id) {
        config.subscription_catalogs.insert(new_id.to_owned(), catalog);
    }
}

/// 某个模型在当前账号与连接世代下的资格。资格必须同时绑定服务商、上游 ID、账号与世代。
pub fn eligibility(config: &AppConfig, provider_id: &str, model_id: &str) -> Eligibility {
    let Some(entry) = catalog(config, provider_id).and_then(|known| known.entry(model_id)) else {
        return Eligibility::NotDiscovered;
    };
    let Some(connection) = config.subscriptions.get(provider_id) else { return Eligibility::AccountChanged };
    let identity = connection.identity.as_deref().map(str::trim).unwrap_or("");
    if identity.is_empty() {
        return Eligibility::AccountChanged;
    }
    if entry.account.as_deref().map(str::trim) != Some(identity) || entry.confirmed_generation != Some(connection.generation)
    {
        return Eligibility::AccountChanged;
    }
    match entry.availability {
        Availability::Available => Eligibility::Eligible,
        Availability::Stale => Eligibility::Stale,
        Availability::Removed => Eligibility::Removed,
        Availability::Revoked => Eligibility::Revoked,
        Availability::Unknown => Eligibility::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subscription::Connection;

    const PROVIDER: &str = "grok-subscription";
    const ACCOUNT: &str = "acct-1";

    /// 最小配置：只保留测试用的订阅连接，不继承默认服务商与模型。
    fn config_with_connection() -> AppConfig {
        let mut config = AppConfig::default();
        config.providers.clear();
        config.models.clear();
        config.routes.clear();
        config.subscriptions.clear();
        config.subscription_catalogs.clear();
        connect(&mut config, ACCOUNT, 1);
        config
    }

    fn connect(config: &mut AppConfig, identity: &str, generation: u64) {
        config.subscriptions.insert(
            PROVIDER.to_owned(),
            Connection { generation, identity: Some(identity.to_owned()), ..Default::default() },
        );
    }

    fn discovery(model_id: &str, name: Option<&str>, eligible: bool) -> DiscoveredModel {
        DiscoveredModel { model_id: model_id.to_owned(), name: name.map(str::to_owned), eligible }
    }

    fn entry(config: &AppConfig, model_id: &str) -> CatalogEntry {
        catalog(config, PROVIDER)
            .and_then(|known| known.entry(model_id))
            .cloned()
            .unwrap_or_else(|| panic!("目录项 {model_id} 应存在"))
    }

    fn model_row(config: &AppConfig, model_id: &str) -> Model {
        config
            .models
            .iter()
            .find(|model| model.provider_id == PROVIDER && model.model_id == model_id)
            .cloned()
            .unwrap_or_else(|| panic!("模型行 {model_id} 应存在"))
    }

    /// 模拟用户在界面上的选择与停用。
    fn set_user_state(config: &mut AppConfig, model_id: &str, selected: bool, enabled: bool) {
        let row = config
            .models
            .iter_mut()
            .find(|model| model.provider_id == PROVIDER && model.model_id == model_id)
            .expect("模型行应存在");
        row.selected = selected;
        row.enabled = enabled;
    }

    /// 旧配置或手工添加的订阅模型行：先有模型行，再触发目录核对。
    fn preexisting_row(config: &mut AppConfig, model_id: &str, internal_id: &str, selected: bool, enabled: bool) {
        config.models.push(Model {
            input_price_known: None,
            output_price_known: None,
            cache_price_known: None,
            api_type: String::new(),
            cache_cost_per_million: 0.0,
            id: internal_id.to_owned(),
            provider_id: PROVIDER.to_owned(),
            model_id: model_id.to_owned(),
            name: "Preexisting".to_owned(),
            tier: ModelTier::Balanced,
            enabled,
            selected,
            supports_tools: true,
            supports_vision: false,
            supports_reasoning: false,
            context_window: 0,
            input_cost_per_million: 0.0,
            output_cost_per_million: 0.0,
        });
    }

    #[test]
    fn reconcile_builds_catalog_and_unselected_model_rows() {
        let mut config = config_with_connection();
        let outcome = reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            1,
            &[discovery("grok-code-fast-1", Some("Grok Code Fast 1"), true), discovery("grok-4", None, true)],
            Some("2026-09-30T00:00:00Z"),
        );
        assert_eq!(outcome.added, vec!["grok-code-fast-1".to_owned(), "grok-4".to_owned()]);
        assert_eq!(catalog(&config, PROVIDER).unwrap().entries.len(), 2, "首次核对必须建立目录");

        let named = entry(&config, "grok-code-fast-1");
        assert_eq!(named.availability, Availability::Available);
        assert_eq!(named.name.as_deref(), Some("Grok Code Fast 1"));
        assert_eq!(named.account.as_deref(), Some(ACCOUNT));
        assert_eq!(named.confirmed_generation, Some(1));
        assert_eq!(named.first_seen.as_deref(), Some("2026-09-30T00:00:00Z"));
        assert_eq!(named.last_confirmed.as_deref(), Some("2026-09-30T00:00:00Z"));

        let unnamed = entry(&config, "grok-4");
        assert_eq!(unnamed.name, None, "上游没有显示名时不编造");
        for model_id in ["grok-code-fast-1", "grok-4"] {
            let row = model_row(&config, model_id);
            assert_eq!(row.id, entry(&config, model_id).internal_id, "模型行标识等于目录内部标识");
            assert!(!row.selected, "新发现模型默认未选");
            assert!(row.enabled, "新发现模型默认未停用");
            assert_eq!(row.name, entry(&config, model_id).name.unwrap_or_else(|| model_id.to_owned()));
        }
        assert_eq!(selected_models(&config, PROVIDER).len(), 0, "未选模型不进入列表与自动候选");
        assert_eq!(models(&config, PROVIDER).len(), 2, "取消选择不移除模型行");
    }

    #[test]
    fn reconcile_keeps_internal_ids_and_user_state_across_runs() {
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("grok-4", Some("Grok 4"), true)], Some("t1"));
        let original = entry(&config, "grok-4").internal_id.clone();
        set_user_state(&mut config, "grok-4", true, false);

        let outcome = reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            2,
            &[discovery("grok-4", Some("Grok 4 (renamed)"), true)],
            Some("t2"),
        );
        assert!(outcome.added.is_empty());
        assert_eq!(outcome.retained, vec!["grok-4".to_owned()]);
        let record = entry(&config, "grok-4");
        assert_eq!(record.internal_id, original, "同一上游 ID 沿用原内部标识");
        assert_eq!(record.name.as_deref(), Some("Grok 4 (renamed)"), "显示名只作展示，可随上游改变");
        assert_eq!(record.confirmed_generation, Some(2), "资格跟随最新核实的世代");
        assert_eq!(config.models.iter().filter(|model| model.model_id == "grok-4").count(), 1, "不重复建档");
        let row = model_row(&config, "grok-4");
        assert!(row.selected && !row.enabled, "核对既不恢复选择也不恢复停用");
        assert_eq!(row.id, original);
    }

    #[test]
    fn upstream_id_change_is_a_new_model_not_a_rename() {
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("grok-4", Some("Grok"), true)], Some("t1"));
        let old_internal = entry(&config, "grok-4").internal_id.clone();
        set_user_state(&mut config, "grok-4", true, false);

        let outcome =
            reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("grok-4-1", Some("Grok"), true)], Some("t2"));
        assert_eq!(outcome.added, vec!["grok-4-1".to_owned()]);
        assert_eq!(outcome.removed, vec!["grok-4".to_owned()], "旧上游 ID 按权威移除处理");

        let fresh = entry(&config, "grok-4-1");
        assert_ne!(fresh.internal_id, old_internal, "上游 ID 变化视为新模型，不按相似显示名重绑");
        assert_eq!(fresh.availability, Availability::Available);
        assert_eq!(entry(&config, "grok-4").availability, Availability::Removed);
        assert_eq!(model_row(&config, "grok-4").id, old_internal);
        let old_row = model_row(&config, "grok-4");
        assert!(old_row.selected && !old_row.enabled, "旧模型保留用户配置");
        let new_row = model_row(&config, "grok-4-1");
        assert_ne!(new_row.id, old_internal);
        assert!(!new_row.selected && new_row.enabled, "新模型不继承同名旧模型的选择与停用");
    }

    #[test]
    fn absent_entries_are_removed_without_touching_user_state() {
        let mut config = config_with_connection();
        reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            1,
            &[discovery("a", None, true), discovery("b", None, true)],
            Some("t1"),
        );
        set_user_state(&mut config, "a", true, true);
        set_user_state(&mut config, "b", false, false);
        let b_internal = entry(&config, "b").internal_id.clone();

        let outcome = reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("a", None, true)], Some("t2"));
        assert_eq!(outcome.removed, vec!["b".to_owned()]);
        assert_eq!(entry(&config, "b").availability, Availability::Removed);
        assert_eq!(catalog(&config, PROVIDER).unwrap().entries.len(), 2, "移除不删目录项、不删模型行");
        let removed_row = model_row(&config, "b");
        assert!(!removed_row.selected && !removed_row.enabled, "移除不改写选择与停用");
        assert!(model_row(&config, "a").selected);

        let again = reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            1,
            &[discovery("a", None, true), discovery("b", None, true)],
            Some("t3"),
        );
        assert!(again.removed.is_empty());
        assert_eq!(again.retained, vec!["a".to_owned(), "b".to_owned()]);
        assert_eq!(entry(&config, "b").internal_id, b_internal, "重新出现沿用原内部标识");
        assert!(!model_row(&config, "b").selected, "重新出现不恢复已取消的选择");
        assert!(!model_row(&config, "b").enabled, "重新出现不恢复停用状态");
    }

    #[test]
    fn reconcile_records_account_revocation_without_losing_the_row() {
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("grok-4", None, false)], Some("t1"));
        assert_eq!(entry(&config, "grok-4").availability, Availability::Revoked);
        assert_eq!(eligibility(&config, PROVIDER, "grok-4"), Eligibility::Revoked);
        assert_eq!(eligibility(&config, PROVIDER, "grok-4").code(), Some("model_revoked"));
        assert!(!eligibility(&config, PROVIDER, "grok-4").is_eligible());
        assert!(model_row(&config, "grok-4").enabled, "资格撤销不是用户停用");
        assert_eq!(config.models.len(), 1, "撤销权限仍保留模型行与配置");

        set_user_state(&mut config, "grok-4", true, true);
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("grok-4", None, true)], Some("t2"));
        assert_eq!(entry(&config, "grok-4").availability, Availability::Available);
        assert_eq!(eligibility(&config, PROVIDER, "grok-4"), Eligibility::Eligible);
        assert!(model_row(&config, "grok-4").selected, "权限恢复保留用户选择");
    }

    #[test]
    fn directory_failure_marks_available_stale_and_keeps_everything() {
        let mut config = config_with_connection();
        reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            1,
            &[discovery("a", None, true), discovery("b", None, false)],
            Some("t1"),
        );
        set_user_state(&mut config, "a", true, true);
        set_user_state(&mut config, "b", true, false);
        let internal = entry(&config, "a").internal_id.clone();

        mark_stale(&mut config, PROVIDER);
        assert_eq!(entry(&config, "a").availability, Availability::Stale);
        assert_eq!(entry(&config, "b").availability, Availability::Revoked, "只把 Available 标为陈旧");
        assert_eq!(catalog(&config, PROVIDER).unwrap().entries.len(), 2, "读取失败不删项");
        let row = model_row(&config, "a");
        assert!(row.selected && row.enabled && row.id == internal, "读取失败不改选择、停用与标识");
        let stopped = model_row(&config, "b");
        assert!(!stopped.enabled, "已停用的模型保持停用");
        assert_eq!(entry(&config, "a").internal_id, internal);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::Stale);
        assert!(eligibility(&config, PROVIDER, "a").is_eligible(), "网络失败不等于模型被移除");
        assert_eq!(eligibility(&config, PROVIDER, "a").code(), None);
        assert_eq!(selected_models(&config, PROVIDER).len(), 1, "陈旧但已选的模型仍在列表");
        assert_eq!(selected_models(&config, PROVIDER)[0].model_id, "a");
    }

    #[test]
    fn account_invalidation_drops_qualification_only() {
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("a", None, true)], Some("t1"));
        let internal = entry(&config, "a").internal_id.clone();
        set_user_state(&mut config, "a", true, false);

        invalidate_account(&mut config, PROVIDER);
        let record = entry(&config, "a");
        assert_eq!(record.availability, Availability::Unknown);
        assert_eq!(record.account, None, "旧账号不得继续构成资格");
        assert_eq!(record.confirmed_generation, None);
        assert_eq!(record.last_confirmed, None);
        assert_eq!(record.internal_id, internal, "退出或换号不重建标识");
        let row = model_row(&config, "a");
        assert!(row.selected && !row.enabled && row.id == internal, "退出或换号不丢选择与停用");
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged);
        assert_eq!(eligibility(&config, PROVIDER, "a").code(), Some("model_unqualified"));

        // 换号重核：同一上游 ID 沿用原标识并恢复资格，用户配置不变
        connect(&mut config, "acct-2", 7);
        reconcile(&mut config, PROVIDER, "acct-2", 7, &[discovery("a", None, true)], Some("t2"));
        let rechecked = entry(&config, "a");
        assert_eq!(rechecked.availability, Availability::Available);
        assert_eq!(rechecked.internal_id, internal);
        assert_eq!(rechecked.account.as_deref(), Some("acct-2"));
        assert_eq!(rechecked.confirmed_generation, Some(7));
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::Eligible);
        let row = model_row(&config, "a");
        assert!(row.selected && !row.enabled, "换号重核保留原选择与停用");
    }

    #[test]
    fn eligibility_binds_model_account_and_generation() {
        let mut config = config_with_connection();
        reconcile(
            &mut config,
            PROVIDER,
            ACCOUNT,
            1,
            &[discovery("a", None, true), discovery("gone", None, true)],
            Some("t1"),
        );
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::Eligible);
        assert_eq!(eligibility(&config, PROVIDER, "a").code(), None);
        assert_eq!(eligibility(&config, PROVIDER, "missing"), Eligibility::NotDiscovered);
        assert_eq!(eligibility(&config, PROVIDER, "missing").code(), Some("model_not_discovered"));
        assert_eq!(eligibility(&config, "other-provider", "a"), Eligibility::NotDiscovered);

        connect(&mut config, "acct-2", 1);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged, "账号不同即未绑定");
        connect(&mut config, ACCOUNT, 2);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged, "世代不同即未绑定");
        connect(&mut config, "  ", 1);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged, "身份为空不算资格");
        config.subscriptions.remove(PROVIDER);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged, "无连接即未绑定");

        connect(&mut config, ACCOUNT, 1);
        mark_stale(&mut config, PROVIDER);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::Stale);
        assert!(eligibility(&config, PROVIDER, "a").is_eligible());
        invalidate_account(&mut config, PROVIDER);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::AccountChanged, "作废后未重核即无资格");
        assert_eq!(eligibility(&config, PROVIDER, "a").code(), Some("model_unqualified"));

        // 账号与世代都匹配但尚无资格依据（手工构造）才是 Unknown
        let stale_entry = config
            .subscription_catalogs
            .get_mut(PROVIDER)
            .and_then(|known| known.entry_mut("a"))
            .expect("目录项应存在");
        stale_entry.availability = Availability::Unknown;
        stale_entry.account = Some(ACCOUNT.to_owned());
        stale_entry.confirmed_generation = Some(1);
        assert_eq!(eligibility(&config, PROVIDER, "a"), Eligibility::Unknown);
        assert!(!eligibility(&config, PROVIDER, "a").is_eligible());
        assert_eq!(eligibility(&config, PROVIDER, "a").code(), Some("model_unqualified"));

        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("a", None, true)], Some("t2"));
        assert_eq!(eligibility(&config, PROVIDER, "gone"), Eligibility::Removed);
        assert_eq!(eligibility(&config, PROVIDER, "gone").code(), Some("model_removed"));
    }

    #[test]
    fn provider_rename_migrates_the_catalog_and_forget_drops_it() {
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("a", None, true)], Some("t1"));
        let internal = entry(&config, "a").internal_id.clone();

        rename_provider(&mut config, PROVIDER, "grok-next");
        assert!(catalog(&config, PROVIDER).is_none(), "旧标识不再保留目录");
        let migrated = catalog(&config, "grok-next").expect("目录随服务商迁移");
        assert_eq!(migrated.entries.len(), 1);
        assert_eq!(migrated.entries[0].internal_id, internal, "迁移不重建标识");
        assert_eq!(migrated.entries[0].availability, Availability::Available);

        rename_provider(&mut config, "grok-next", "grok-next");
        assert!(catalog(&config, "grok-next").is_some(), "同名重命名是空操作");

        forget_provider(&mut config, "grok-next");
        assert!(catalog(&config, "grok-next").is_none());
        assert_eq!(model_row(&config, "a").id, internal, "目录删除不影响模型行");
    }

    #[test]
    fn legacy_json_defaults_and_full_round_trip_preserve_state() {
        // 旧配置：没有 subscription_catalogs，模型行也没有 selected 字段
        let mut legacy = serde_json::to_value(AppConfig::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("subscription_catalogs");
        legacy["models"][0].as_object_mut().unwrap().remove("selected");
        let restored: AppConfig = serde_json::from_value(legacy).unwrap();
        assert!(restored.subscription_catalogs.is_empty(), "旧配置没有目录时为空");
        assert!(restored.models[0].selected, "旧配置与 API 模型默认已选");

        // 新配置完整往返：选择、停用、目录资格与稳定标识都保持
        let mut config = config_with_connection();
        reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("a", Some("A"), true)], Some("t1"));
        set_user_state(&mut config, "a", true, false);
        let json = serde_json::to_string(&config).unwrap();
        let reloaded: AppConfig = serde_json::from_str(&json).unwrap();

        let record = catalog(&reloaded, PROVIDER).and_then(|known| known.entry("a")).cloned().unwrap();
        assert_eq!(record.availability, Availability::Available);
        assert_eq!(record.internal_id, entry(&config, "a").internal_id);
        assert_eq!(record.confirmed_generation, Some(1));
        assert_eq!(record.account.as_deref(), Some(ACCOUNT));
        assert_eq!(record.first_seen.as_deref(), Some("t1"));
        let row = model_row(&reloaded, "a");
        assert!(row.selected && !row.enabled, "往返后选择与停用保持");
        assert_eq!(row.id, record.internal_id);
        assert_eq!(eligibility(&reloaded, PROVIDER, "a"), Eligibility::Eligible);
    }

    #[test]
    fn reconcile_adopts_identity_of_preexisting_model_row() {
        let mut config = config_with_connection();
        preexisting_row(&mut config, "fixture-model", "codex-model", true, true);

        let outcome =
            reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("fixture-model", None, true)], Some("t1"));
        assert_eq!(outcome.added, vec!["fixture-model".to_owned()]);
        assert_eq!(config.models.len(), 1, "不重复物化模型行");
        let record = entry(&config, "fixture-model");
        assert_eq!(record.internal_id, "codex-model", "目录标识必须沿用既有模型行 id");
        assert_eq!(record.availability, Availability::Available);
        let row = model_row(&config, "fixture-model");
        assert_eq!(row.id, "codex-model", "不覆盖既有模型行");
        assert!(row.selected && row.enabled, "沿用既有模型行的选择与停用");
        assert_eq!(eligibility(&config, PROVIDER, "fixture-model"), Eligibility::Eligible);
        assert_eq!(selected_models(&config, PROVIDER)[0].id, record.internal_id, "按目录标识能找到模型行");
    }

    #[test]
    fn reconcile_repairs_entry_identity_that_diverged_from_the_model_row() {
        let mut config = config_with_connection();
        preexisting_row(&mut config, "fixture-model", "codex-model", true, true);
        // 模拟早前版本留下的不一致：目录条目指向一个不存在的内部标识。
        config.subscription_catalogs.insert(
            PROVIDER.to_owned(),
            ProviderCatalog {
                entries: vec![CatalogEntry {
                    model_id: "fixture-model".to_owned(),
                    name: None,
                    internal_id: "orphan-uuid".to_owned(),
                    availability: Availability::Stale,
                    first_seen: Some("t0".to_owned()),
                    last_confirmed: Some("t0".to_owned()),
                    confirmed_generation: Some(0),
                    account: Some("old-account".to_owned()),
                }],
            },
        );

        let outcome =
            reconcile(&mut config, PROVIDER, ACCOUNT, 1, &[discovery("fixture-model", None, true)], Some("t1"));
        assert_eq!(outcome.retained, vec!["fixture-model".to_owned()]);
        assert_eq!(config.models.len(), 1);
        let record = entry(&config, "fixture-model");
        assert_eq!(record.internal_id, "codex-model", "核对修正与模型行不一致的标识");
        assert_eq!(model_row(&config, "fixture-model").id, "codex-model");
        assert_eq!(record.account.as_deref(), Some(ACCOUNT));
        assert_eq!(record.confirmed_generation, Some(1));
    }
}
