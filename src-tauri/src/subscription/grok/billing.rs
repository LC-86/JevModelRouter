//! Offline parser and DTOs for the candidate xAI ACP billing response.
//!
//! This module deliberately has no transport, authentication, persistence, or admission hooks.
//! The only native UI entry point is compiled into the explicit `isolation-check` build.

use chrono::DateTime;
use serde::Serialize;
use serde_json::Value;

const LEGACY_USAGE_ORIGIN: &str = "legacy.used/monthlyLimit";
const DIRECT_USAGE_ORIGIN: &str = "config.creditUsagePercent";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldState {
    Available,
    Missing,
    Null,
    Invalid,
    OutOfRange,
}

/// A value keeps transport omission, explicit null, malformed input, and valid zero distinct.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed<T> {
    pub state: FieldState,
    pub value: Option<T>,
    pub origin: Option<String>,
}

impl<T> Observed<T> {
    fn state(state: FieldState) -> Self {
        Self {
            state,
            value: None,
            origin: None,
        }
    }

    fn available(value: T, origin: impl Into<String>) -> Self {
        Self {
            state: FieldState::Available,
            value: Some(value),
            origin: Some(origin.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CentDto {
    /// Raw integer cents. The parser does not convert this to a currency or decimal amount.
    pub val: Observed<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
/// Mirrors the candidate current-period shape; amount fields belong to `history` records.
pub struct UsagePeriodDto {
    pub period_type: Observed<String>,
    pub start: Observed<String>,
    pub end: Observed<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingConfigDto {
    pub credit_usage_percent: Observed<f64>,
    pub current_period: Observed<UsagePeriodDto>,
    pub monthly_limit: Observed<CentDto>,
    pub used: Observed<CentDto>,
    pub on_demand_cap: Observed<CentDto>,
    pub on_demand_used: Observed<CentDto>,
    pub prepaid_balance: Observed<CentDto>,
    pub is_unified_billing_user: Observed<bool>,
    pub billing_period_start: Observed<String>,
    pub billing_period_end: Observed<String>,
    /// The pinned source does not define history item semantics; retain each returned JSON item.
    pub history: Observed<Vec<Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingResponseDto {
    pub response_state: FieldState,
    pub config: Observed<BillingConfigDto>,
    /// These response-level keys are snake_case in the pinned official source.
    pub on_demand_enabled: Observed<bool>,
    pub subscription_tier: Observed<String>,
    pub usage_percent: Observed<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoTopupRuleDto {
    pub enabled: Observed<bool>,
    pub min_before_hitting_sl: Observed<CentDto>,
    pub topup_amount: Observed<CentDto>,
    pub max_amount_per_month: Observed<CentDto>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillingScope {
    pub provider_id: String,
    pub connection_instance_id: String,
    pub generation: u64,
}

/// Reject cached or late values unless provider, connection instance, and generation still match.
/// Account switching advances the connection generation; no account identifier is needed here.
pub fn accept_scoped_response<T>(
    response_scope: &BillingScope,
    current_scope: &BillingScope,
    response: T,
) -> Option<T> {
    (response_scope == current_scope).then_some(response)
}

pub fn parse_billing_response(value: &Value) -> BillingResponseDto {
    let response_state = if value.is_object() {
        FieldState::Available
    } else {
        FieldState::Invalid
    };
    let config = field(value, "config", parse_config);
    let on_demand_enabled = field(value, "on_demand_enabled", parse_bool);
    let subscription_tier = field(value, "subscription_tier", parse_non_empty_string);
    let usage_percent = config
        .value
        .as_ref()
        .map(|config| {
            effective_usage_percent(
                &config.credit_usage_percent,
                &config.used,
                &config.monthly_limit,
            )
        })
        .unwrap_or_else(|| Observed::state(config.state));

    BillingResponseDto {
        response_state,
        config,
        on_demand_enabled,
        subscription_tier,
        usage_percent,
    }
}

/// Parser for the separately returned candidate method. It has no production caller by design.
#[allow(dead_code)]
pub fn parse_auto_topup_response(value: &Value) -> Observed<AutoTopupRuleDto> {
    field(value, "rule", parse_auto_topup_rule)
}

fn field<T>(
    parent: &Value,
    key: &str,
    parse: impl FnOnce(&Value) -> Result<T, FieldState>,
) -> Observed<T> {
    let Some(object) = parent.as_object() else {
        return Observed::state(FieldState::Invalid);
    };
    match object.get(key) {
        None => Observed::state(FieldState::Missing),
        Some(Value::Null) => Observed::state(FieldState::Null),
        Some(value) => match parse(value) {
            Ok(value) => Observed::available(value, key),
            Err(state) => Observed::state(state),
        },
    }
}

fn parse_bool(value: &Value) -> Result<bool, FieldState> {
    value.as_bool().ok_or(FieldState::Invalid)
}

fn parse_non_empty_string(value: &Value) -> Result<String, FieldState> {
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or(FieldState::Invalid)
}

fn parse_rfc3339_timestamp(value: &Value) -> Result<String, FieldState> {
    let Some(value) = value.as_str() else {
        return Err(FieldState::Invalid);
    };
    DateTime::parse_from_rfc3339(value).map_err(|_| FieldState::Invalid)?;
    Ok(value.to_owned())
}

fn parse_percent(value: &Value) -> Result<f64, FieldState> {
    let Some(value) = value.as_f64() else {
        return Err(FieldState::Invalid);
    };
    if !value.is_finite() {
        return Err(FieldState::Invalid);
    }
    if !(0.0..=100.0).contains(&value) {
        return Err(FieldState::OutOfRange);
    }
    Ok(value)
}

fn parse_cent(value: &Value) -> Result<CentDto, FieldState> {
    if !value.is_object() {
        return Err(FieldState::Invalid);
    }
    let val = field(value, "val", |raw| {
        let Some(integer) = raw.as_i64() else {
            return Err(FieldState::Invalid);
        };
        if integer < 0 {
            return Err(FieldState::OutOfRange);
        }
        Ok(integer)
    });
    Ok(CentDto { val })
}

fn parse_usage_period(value: &Value) -> Result<UsagePeriodDto, FieldState> {
    if !value.is_object() {
        return Err(FieldState::Invalid);
    }
    Ok(UsagePeriodDto {
        period_type: field(value, "type", parse_non_empty_string),
        start: field(value, "start", parse_rfc3339_timestamp),
        end: field(value, "end", parse_rfc3339_timestamp),
    })
}

fn parse_config(value: &Value) -> Result<BillingConfigDto, FieldState> {
    if !value.is_object() {
        return Err(FieldState::Invalid);
    }
    let mut credit_usage_percent = field(value, "creditUsagePercent", parse_percent);
    if credit_usage_percent.state == FieldState::Available {
        credit_usage_percent.origin = Some(DIRECT_USAGE_ORIGIN.into());
    }
    Ok(BillingConfigDto {
        credit_usage_percent,
        current_period: field(value, "currentPeriod", parse_usage_period),
        monthly_limit: field(value, "monthlyLimit", parse_cent),
        used: field(value, "used", parse_cent),
        on_demand_cap: field(value, "onDemandCap", parse_cent),
        on_demand_used: field(value, "onDemandUsed", parse_cent),
        prepaid_balance: field(value, "prepaidBalance", parse_cent),
        is_unified_billing_user: field(value, "isUnifiedBillingUser", parse_bool),
        billing_period_start: field(value, "billingPeriodStart", parse_rfc3339_timestamp),
        billing_period_end: field(value, "billingPeriodEnd", parse_rfc3339_timestamp),
        history: field(value, "history", |raw| {
            raw.as_array().cloned().ok_or(FieldState::Invalid)
        }),
    })
}

#[allow(dead_code)]
fn parse_auto_topup_rule(value: &Value) -> Result<AutoTopupRuleDto, FieldState> {
    if !value.is_object() {
        return Err(FieldState::Invalid);
    }
    Ok(AutoTopupRuleDto {
        enabled: field(value, "enabled", parse_bool),
        min_before_hitting_sl: field(value, "minBeforeHittingSl", parse_cent),
        topup_amount: field(value, "topupAmount", parse_cent),
        max_amount_per_month: field(value, "maxAmountPerMonth", parse_cent),
    })
}

fn effective_usage_percent(
    direct: &Observed<f64>,
    used: &Observed<CentDto>,
    limit: &Observed<CentDto>,
) -> Observed<f64> {
    if direct.state == FieldState::Available {
        return direct.clone();
    }
    if !matches!(direct.state, FieldState::Missing | FieldState::Null) {
        return Observed::state(direct.state);
    }
    let Some(used) = legacy_cent_value(used) else {
        return legacy_cent_error(used);
    };
    let Some(limit) = legacy_cent_value(limit) else {
        return legacy_cent_error(limit);
    };
    if limit <= 0 {
        return Observed::state(FieldState::OutOfRange);
    }
    if used > limit {
        return Observed::state(FieldState::OutOfRange);
    }
    let percent = (used as f64 / limit as f64) * 100.0;
    if !(0.0..=100.0).contains(&percent) {
        return Observed::state(FieldState::OutOfRange);
    }
    Observed::available(percent, LEGACY_USAGE_ORIGIN)
}

fn legacy_cent_value(field: &Observed<CentDto>) -> Option<i64> {
    field.value.as_ref().and_then(|cent| cent.val.value)
}

fn legacy_cent_error(field: &Observed<CentDto>) -> Observed<f64> {
    Observed::state(
        field
            .value
            .as_ref()
            .map(|cent| cent.val.state)
            .unwrap_or(field.state),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_pinned_camel_case_config_and_snake_case_response_fields() {
        let response = parse_billing_response(&json!({
            "config": {
                "creditUsagePercent": 0,
                "currentPeriod": {
                    "type":"USAGE_PERIOD_TYPE_MONTHLY",
                    "start":"2026-10-01T00:00:00Z",
                    "end":"2026-11-01T00:00:00Z"
                },
                "monthlyLimit":{"val":100}, "used":{"val":0}, "onDemandCap":{"val":4},
                "onDemandUsed":{"val":2}, "prepaidBalance":{"val":8},
                "isUnifiedBillingUser":false,
                "billingPeriodStart":"2026-10-01T00:00:00Z",
                "billingPeriodEnd":"2026-11-01T00:00:00Z",
                "history":[{
                    "billingCycle":{"year":2026,"month":9},
                    "includedUsed":{"val":0},
                    "onDemandUsed":{"val":0},
                    "totalUsed":{"val":0},
                    "future":true
                }]
            },
            "on_demand_enabled":false,
            "subscription_tier":"fixture-plan",
            "onDemandEnabled":true,
            "subscriptionTier":"wrong-case"
        }));
        assert_eq!(response.response_state, FieldState::Available);
        assert_eq!(response.on_demand_enabled.value, Some(false));
        assert_eq!(
            response.subscription_tier.value.as_deref(),
            Some("fixture-plan")
        );
        assert_eq!(
            response.usage_percent.value,
            Some(0.0),
            "explicit zero is available"
        );
        assert_eq!(
            response.usage_percent.origin.as_deref(),
            Some(DIRECT_USAGE_ORIGIN)
        );
        let config = response.config.value.unwrap();
        assert_eq!(config.is_unified_billing_user.value, Some(false));
        let current_period = config.current_period.value.unwrap();
        assert_eq!(
            current_period.period_type.value.as_deref(),
            Some("USAGE_PERIOD_TYPE_MONTHLY")
        );
        assert_eq!(
            current_period.start.value.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(
            current_period.end.value.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        assert_eq!(
            config.history.value.as_ref().unwrap()[0]["billingCycle"]["month"],
            9
        );
        assert_eq!(
            config.history.value.as_ref().unwrap()[0]["includedUsed"]["val"],
            0
        );
        assert_eq!(config.history.value.unwrap()[0]["future"], true);
    }

    #[test]
    fn preserves_missing_null_invalid_and_out_of_range_without_fabricating_zero() {
        let response = parse_billing_response(&json!({
            "config": {
                "creditUsagePercent": 101,
                "currentPeriod": null,
                "monthlyLimit": {"val": null},
                "used": {"val": "12"},
                "onDemandCap": {"val": -1},
                "onDemandUsed": {"val": 2.5},
                "prepaidBalance": null,
                "isUnifiedBillingUser": "false",
                "history": {}
            },
            "on_demand_enabled": null
        }));
        let config = response.config.value.unwrap();
        assert_eq!(config.credit_usage_percent.state, FieldState::OutOfRange);
        assert_eq!(config.current_period.state, FieldState::Null);
        assert_eq!(
            config.monthly_limit.value.unwrap().val.state,
            FieldState::Null
        );
        assert_eq!(config.used.value.unwrap().val.state, FieldState::Invalid);
        assert_eq!(
            config.on_demand_cap.value.unwrap().val.state,
            FieldState::OutOfRange
        );
        assert_eq!(
            config.on_demand_used.value.unwrap().val.state,
            FieldState::Invalid
        );
        assert_eq!(config.prepaid_balance.state, FieldState::Null);
        assert_eq!(config.is_unified_billing_user.state, FieldState::Invalid);
        assert_eq!(config.history.state, FieldState::Invalid);
        assert_eq!(response.on_demand_enabled.state, FieldState::Null);
        assert_eq!(response.subscription_tier.state, FieldState::Missing);
        assert_eq!(response.usage_percent.state, FieldState::OutOfRange);
    }

    #[test]
    fn period_timestamps_require_valid_rfc3339_and_preserve_missing_null_and_invalid() {
        let invalid = parse_billing_response(&json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_MONTHLY",
                    "start": "not-a-date",
                    "end": "2026-02-30T00:00:00Z"
                },
                "billingPeriodStart": "start",
                "billingPeriodEnd": "2026-02-30T00:00:00Z"
            }
        }));
        let invalid_config = invalid.config.value.unwrap();
        let invalid_period = invalid_config.current_period.value.unwrap();
        assert_eq!(invalid_period.start.state, FieldState::Invalid);
        assert_eq!(invalid_period.end.state, FieldState::Invalid);
        assert_eq!(
            invalid_config.billing_period_start.state,
            FieldState::Invalid
        );
        assert_eq!(invalid_config.billing_period_end.state, FieldState::Invalid);

        let sparse = parse_billing_response(&json!({
            "config": {
                "currentPeriod": {"type":"USAGE_PERIOD_TYPE_MONTHLY", "end":null},
                "billingPeriodStart":null
            }
        }));
        let sparse_config = sparse.config.value.unwrap();
        let sparse_period = sparse_config.current_period.value.unwrap();
        assert_eq!(sparse_period.start.state, FieldState::Missing);
        assert_eq!(sparse_period.end.state, FieldState::Null);
        assert_eq!(sparse_config.billing_period_start.state, FieldState::Null);
        assert_eq!(sparse_config.billing_period_end.state, FieldState::Missing);

        let valid = parse_billing_response(&json!({
            "config": {
                "currentPeriod": {
                    "type":"USAGE_PERIOD_TYPE_MONTHLY",
                    "start":"2024-02-29T10:15:30.125+02:30",
                    "end":"2024-03-01T00:00:00Z"
                },
                "billingPeriodStart":"2024-02-29T00:00:00Z",
                "billingPeriodEnd":"2024-03-01T00:00:00Z"
            }
        }));
        let valid_config = valid.config.value.unwrap();
        let valid_period = valid_config.current_period.value.unwrap();
        assert_eq!(valid_period.start.state, FieldState::Available);
        assert_eq!(
            valid_period.start.value.as_deref(),
            Some("2024-02-29T10:15:30.125+02:30")
        );
        assert_eq!(valid_period.end.state, FieldState::Available);
        assert_eq!(
            valid_config.billing_period_start.state,
            FieldState::Available
        );
        assert_eq!(valid_config.billing_period_end.state, FieldState::Available);
    }

    #[test]
    fn legacy_ratio_requires_both_valid_values_and_reports_its_origin() {
        let valid = parse_billing_response(
            &json!({"config":{"monthlyLimit":{"val":200},"used":{"val":75}}}),
        );
        assert_eq!(valid.usage_percent.value, Some(37.5));
        assert_eq!(
            valid.usage_percent.origin.as_deref(),
            Some(LEGACY_USAGE_ORIGIN)
        );

        let partial = parse_billing_response(&json!({"config":{"used":{"val":75}}}));
        assert_eq!(partial.usage_percent.state, FieldState::Missing);
        assert_eq!(partial.usage_percent.value, None);

        let malformed_used = parse_billing_response(
            &json!({"config":{"monthlyLimit":{"val":100},"used":{"val":"75"}}}),
        );
        assert_eq!(malformed_used.usage_percent.state, FieldState::Invalid);
        assert_eq!(malformed_used.usage_percent.value, None);

        let null_limit = parse_billing_response(
            &json!({"config":{"monthlyLimit":{"val":null},"used":{"val":75}}}),
        );
        assert_eq!(null_limit.usage_percent.state, FieldState::Null);
        assert_eq!(null_limit.usage_percent.value, None);

        let null =
            parse_billing_response(&json!({"config":{"monthlyLimit":null,"used":{"val":75}}}));
        assert_eq!(null.usage_percent.state, FieldState::Null);

        let zero_limit =
            parse_billing_response(&json!({"config":{"monthlyLimit":{"val":0},"used":{"val":0}}}));
        assert_eq!(zero_limit.usage_percent.state, FieldState::OutOfRange);

        let over_limit = parse_billing_response(
            &json!({"config":{"monthlyLimit":{"val":50},"used":{"val":75}}}),
        );
        assert_eq!(over_limit.usage_percent.state, FieldState::OutOfRange);

        let large_over_limit = parse_billing_response(&json!({
            "config": {
                "monthlyLimit": {"val": 9223372036854775806_i64},
                "used": {"val": 9223372036854775807_i64}
            }
        }));
        assert_eq!(large_over_limit.usage_percent.state, FieldState::OutOfRange);
    }

    #[test]
    fn direct_bad_percent_does_not_fall_back_to_legacy_ratio() {
        let response = parse_billing_response(&json!({
            "config":{"creditUsagePercent":"42","monthlyLimit":{"val":100},"used":{"val":10}}
        }));
        assert_eq!(response.usage_percent.state, FieldState::Invalid);
        assert_eq!(response.usage_percent.value, None);
    }

    #[test]
    fn auto_topup_omissions_are_not_proto_defaulted_and_partial_failure_stays_separate() {
        let rule = parse_auto_topup_response(&json!({"rule":{"topupAmount":{"val":25}}}))
            .value
            .unwrap();
        assert_eq!(rule.enabled.state, FieldState::Missing);
        assert_eq!(rule.topup_amount.value.unwrap().val.value, Some(25));
        assert_eq!(rule.min_before_hitting_sl.state, FieldState::Missing);

    }

    #[test]
    fn scoped_response_is_rejected_after_account_or_connection_switch() {
        let original = BillingScope {
            provider_id: "grok".into(),
            connection_instance_id: "conn-a".into(),
            generation: 7,
        };
        let switched_account = BillingScope {
            generation: 8,
            ..original.clone()
        };
        let switched_connection = BillingScope {
            connection_instance_id: "conn-b".into(),
            ..original.clone()
        };
        let switched_provider = BillingScope {
            provider_id: "other".into(),
            ..original.clone()
        };
        assert_eq!(accept_scoped_response(&original, &original, 42), Some(42));
        assert_eq!(
            accept_scoped_response(&original, &switched_account, 42),
            None
        );
        assert_eq!(
            accept_scoped_response(&original, &switched_connection, 42),
            None
        );
        assert_eq!(
            accept_scoped_response(&original, &switched_provider, 42),
            None
        );
    }
}
