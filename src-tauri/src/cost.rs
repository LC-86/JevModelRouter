//! One conservative cost estimate for routing and explanations. Unknown is never free.
use crate::{config::{AppConfig, Model}, router::RoutePreviewInput};

pub fn known(model: &Model) -> bool {
    model.input_price_known.unwrap_or(model.input_cost_per_million > 0.0)
        && model.output_price_known.unwrap_or(model.output_cost_per_million > 0.0)
}
pub fn cache_known(model: &Model) -> bool {
    model.cache_price_known.unwrap_or(model.cache_cost_per_million > 0.0)
}
fn band(tokens: u64) -> u8 { if tokens < 4096 { 0 } else if tokens < 32768 { 1 } else { 2 } }
fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() { return None; }
    values.sort_by(f64::total_cmp);
    Some(values[values.len()/2])
}
pub fn estimate(config: &AppConfig, model: &Model, input: &RoutePreviewInput) -> Option<f64> {
    if !known(model) { return None; }
    let provider = config.providers.iter().find(|p| p.id == model.provider_id)?;
    let fingerprint = crate::performance::fingerprint(model, provider);
    // Compare the same estimated workload across all candidates. Per-model
    // sample coverage must not give unmeasured models an artificially short reply.
    let comparable: Vec<_> = config.cost_history.iter().filter(|log|
        log.status == "success"
        && log.performance_requires_vision == input.requires_vision
        && log.endpoint == input.endpoint && log.requested_model == input.requested_model.as_deref().unwrap_or("")
        && band(log.performance_context_tokens) == band(input.estimated_context_tokens)
        && log.input_tokens.is_some() && log.output_tokens.is_some()
        && config.models.iter().any(|m| m.id == log.performance_model_id
            && config.providers.iter().find(|p| p.id == m.provider_id)
                .is_some_and(|p| crate::performance::fingerprint(m,p) == log.performance_fingerprint))
    ).collect();
    let output_samples: Vec<_> = comparable.iter().take(20).map(|s| s.output_tokens.unwrap() as f64).collect();
    let output = if output_samples.len() >= 3 {
        median(output_samples).unwrap()
    } else { (input.estimated_context_tokens as f64 * 0.18).max(16.0) };
    let output = output.min(config.output_limit.unwrap_or(u64::MAX) as f64);
    let samples: Vec<_> = comparable.into_iter().filter(|log|
        log.performance_model_id == model.id && log.performance_fingerprint == fingerprint
    ).take(20).collect();
    // Only the incumbent can plausibly reuse the ongoing conversation's cache.
    // Historical hit rates are estimates, not a promise of a hit on this request.
    let cache_ratio = if config.cost_incumbent.as_deref() == Some(&model.id) && cache_known(model) && samples.len() >= 3 {
        median(samples.iter().map(|s| s.cache_read_tokens.min(s.input_tokens.unwrap()) as f64 / s.input_tokens.unwrap().max(1) as f64).collect()).unwrap().clamp(0.0, 1.0)
    } else { 0.0 };
    Some((input.estimated_context_tokens as f64 * ((1.0-cache_ratio)*model.input_cost_per_million + cache_ratio*model.cache_cost_per_million)
        + output*model.output_cost_per_million)/1_000_000.0)
}
pub fn is_cost_route(config: &AppConfig, input: &RoutePreviewInput) -> bool {
    let id = input.requested_model.as_deref().unwrap_or("").strip_prefix("autojev/").unwrap_or("");
    if let Some(route) = config.routes.iter().find(|r| r.id == id && r.enabled) {
        return route.strategy == "jev" && route.automatic_policy.as_ref().unwrap_or(&config.policy).decision_preference == "cost";
    }
    (id == "auto" || id.is_empty()) && config.policy.decision_preference == "cost"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::{ModelTier, RoutingMode}, traffic::RequestLog};
    fn setup() -> (AppConfig, RoutePreviewInput) {
        let mut config=AppConfig::default();
        config.policy.mode=RoutingMode::Auto;
        config.policy.decision_preference="cost".into();
        config.policy.prefer_local=false;
        for m in &mut config.models { m.tier=ModelTier::Fast; }
        let input=RoutePreviewInput{prompt:"hello".into(), endpoint:"chat/completions".into(),requires_tools:false,requires_vision:false,estimated_context_tokens:1000,requested_model:Some("autojev/auto".into())};
        (config,input)
    }
    fn history(config:&mut AppConfig,input:&RoutePreviewInput,cached:u64,output:u64) {
        let model=&config.models[0];
        let fingerprint=crate::performance::fingerprint(model,&config.providers[0]);
        config.cost_history=(0..3).map(|_|RequestLog{status:"success".into(),performance_model_id:model.id.clone(),performance_fingerprint:fingerprint.clone(),performance_context_tokens:1000,endpoint:input.endpoint.clone(),requested_model:input.requested_model.clone().unwrap(),input_tokens:Some(1000),output_tokens:Some(output),cache_read_tokens:cached,..Default::default()}).collect();
    }
    #[test]
    fn missing_partial_and_explicit_free_prices() {
        let (mut c,i)=setup();
        c.models[0].input_cost_per_million=0.0;
        assert_eq!(estimate(&c,&c.models[0],&i),None);
        c.models[0].input_price_known=Some(true);
        assert!(estimate(&c,&c.models[0],&i).unwrap()>0.0);
        c.models[0].output_cost_per_million=0.0;
        c.models[0].output_price_known=Some(true);
        assert_eq!(estimate(&c,&c.models[0],&i),Some(0.0));
        c.models[0].output_price_known=Some(false);
        assert_eq!(estimate(&c,&c.models[0],&i),None);
    }
    #[test]
    fn history_output_caps_and_cache_are_conservative() {
        let (mut c,i)=setup();
        c.models[0].input_cost_per_million=1.0;c.models[0].output_cost_per_million=2.0;
        history(&mut c,&i,800,400);
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.0018).abs()<1e-9);
        c.output_limit=Some(100);
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.0012).abs()<1e-9);
        c.cost_incumbent=Some(c.models[0].id.clone());
        // Unknown cache rate is not a free cache hit.
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.0012).abs()<1e-9);
        c.models[0].cache_price_known=Some(true);
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.0004).abs()<1e-9);
        c.cost_incumbent=None;
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.0012).abs()<1e-9);
        c.cost_history.truncate(2);c.output_limit=None;
        assert!((estimate(&c,&c.models[0],&i).unwrap()-0.00136).abs()<1e-9);
    }
    #[tokio::test]
    async fn deterministic_price_choice_unknown_fallback_and_session_threshold() {
        let (mut c,i)=setup();let client=reqwest::Client::new();
        c.models[0].input_cost_per_million=0.0;c.models[0].output_cost_per_million=0.0;
        assert_eq!(crate::router::decide(&c,&i,&client,None).await.unwrap().model.id,c.models[1].id);
        c.models[0].input_price_known=Some(true);c.models[0].output_price_known=Some(true);
        assert_eq!(crate::router::decide(&c,&i,&client,None).await.unwrap().model.id,c.models[0].id);
        c.models[0].input_cost_per_million=0.9;c.models[0].output_cost_per_million=0.9;
        c.models[1].input_cost_per_million=1.0;c.models[1].output_cost_per_million=1.0;
        c.cost_incumbent=Some(c.models[1].id.clone());
        assert_eq!(crate::router::decide(&c,&i,&client,None).await.unwrap().model.id,c.models[1].id);
        c.models[0].input_cost_per_million=0.5;c.models[0].output_cost_per_million=0.5;
        assert_eq!(crate::router::decide(&c,&i,&client,None).await.unwrap().model.id,c.models[0].id);
        c.models[0].input_price_known=Some(false);c.models[1].input_price_known=Some(false);
        assert!(crate::router::decide(&c,&i,&client,None).await.unwrap().decision.estimated_cost.is_none());
    }
    #[tokio::test]
    async fn cheap_incapable_models_do_not_win() {
        let (mut c,mut i)=setup();i.requires_tools=true;
        c.models[0].supports_tools=false;
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[1].id);
        c.models[1].context_window=100;
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[1].id);
    }
    #[tokio::test]
    async fn incumbent_cache_can_outweigh_lower_sticker_price() {
        let (mut c,i)=setup();
        c.models[0].input_cost_per_million=1.0;c.models[0].output_cost_per_million=1.0;
        c.models[0].cache_cost_per_million=0.1;
        c.models[1].input_cost_per_million=0.5;c.models[1].output_cost_per_million=0.5;
        history(&mut c,&i,900,180);
        c.cost_incumbent=Some(c.models[0].id.clone());
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[0].id);
        c.cost_incumbent=None;
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[1].id);
    }
    #[tokio::test]
    async fn explicit_model_selection_is_not_overridden_by_cost_policy() {
        let (mut c,mut i)=setup();
        c.models[0].supports_vision=true;i.requires_vision=true;
        i.requested_model=Some(format!("autojev/model/{}",c.models[0].id));
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[0].id);
    }
    #[test]
    fn explicit_zero_and_unknown_survive_serialization() {
        let (mut c,_)=setup();
        c.models[0].input_price_known=Some(true);c.models[0].output_price_known=Some(true);
        c.models[0].input_cost_per_million=0.0;c.models[0].output_cost_per_million=0.0;
        let saved:AppConfig=serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert!(known(&saved.models[0]));
        let mut legacy=serde_json::to_value(&c.models[0]).unwrap();
        legacy.as_object_mut().unwrap().remove("input_price_known");
        legacy.as_object_mut().unwrap().remove("output_price_known");
        assert!(!known(&serde_json::from_value(legacy.clone()).unwrap()));
        legacy.as_object_mut().unwrap().remove("input_cost_per_million");
        legacy.as_object_mut().unwrap().remove("output_cost_per_million");
        assert!(!known(&serde_json::from_value(legacy).unwrap()));
    }

    #[tokio::test]
    async fn uneven_sample_coverage_cannot_make_higher_rates_cheaper() {
        let (mut c,mut i)=setup();
        i.estimated_context_tokens=25;
        c.models[0].input_cost_per_million=0.12;c.models[0].output_cost_per_million=0.48;
        c.models[1].input_cost_per_million=0.15;c.models[1].output_cost_per_million=0.6;
        history(&mut c,&i,0,171);
        for sample in &mut c.cost_history { sample.performance_context_tokens=25; }
        let cheap=estimate(&c,&c.models[0],&i).unwrap();
        let expensive=estimate(&c,&c.models[1],&i).unwrap();
        assert!((cheap-0.00008508).abs()<1e-10);
        assert!((expensive-0.00010635).abs()<1e-10);
        assert_eq!(crate::router::decide(&c,&i,&reqwest::Client::new(),None).await.unwrap().model.id,c.models[0].id);
        // Both sides retain a common estimate even after the other model acquires history.
        let mut other=c.cost_history[0].clone();
        other.performance_model_id=c.models[1].id.clone();
        other.performance_fingerprint=crate::performance::fingerprint(&c.models[1],&c.providers[0]);
        other.output_tokens=Some(16);
        c.cost_history.insert(0,other);
        assert!(estimate(&c,&c.models[0],&i).unwrap()<estimate(&c,&c.models[1],&i).unwrap());
    }

    #[tokio::test]
    async fn cost_routing_requires_image_support_but_context_remains_advisory() {
        let (mut c,mut i)=setup();
        i.requires_vision=true;
        for model in &mut c.models { model.tier=ModelTier::Strong; model.supports_vision=false; }
        c.models[1].supports_vision=true;
        c.models[1].context_window=1;
        let client=reqwest::Client::new();
        assert_eq!(crate::router::decide(&c,&i,&client,None).await.unwrap().model.id,c.models[1].id);
        c.models[1].supports_vision=false;
        assert!(crate::router::decide(&c,&i,&client,None).await.is_err());
    }

}
