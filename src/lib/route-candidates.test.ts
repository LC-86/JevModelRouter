import { describe, expect, it } from 'vitest';
import type { Model, Provider, RouteRule } from '../types';
import {
  connectableRoutes, isRouteCandidate, protocolCompatible, routeCandidateModels, routeHasCandidates, routeIncludesModel,
} from './route-candidates';

const apiProvider: Provider = {
  id: 'openrouter', name: 'OpenRouter', kind: 'openrouter', base_url: '', enabled: true, has_api_key: false, api_type: 'chat_completions',
};
const subscriptionProvider: Provider = {
  id: 'codex', name: 'Codex', kind: 'codex_subscription', base_url: '', enabled: true, has_api_key: false, api_type: 'responses',
};

function model(patch: Partial<Model> = {}): Model {
  return {
    id: 'api-selected', provider_id: 'openrouter', model_id: 'vendor/selected', name: 'Selected',
    tier: 'balanced', enabled: true, selected: true, supports_tools: true, supports_vision: false,
    supports_reasoning: false, context_window: 128000, input_cost_per_million: 0, output_cost_per_million: 0,
    ...patch,
  };
}

const allModelsRoute: RouteRule = { id: 'everything', name: 'Everything', strategy: 'jev', all_models: true, model_ids: [], enabled: true };
const explicitRoute: RouteRule = { id: 'explicit', name: 'Explicit', strategy: 'round_robin', all_models: false, model_ids: ['subscription-selected'], enabled: true };

const subscriptionModel = model({ id: 'subscription-selected', provider_id: 'codex', model_id: 'codex-alpha', name: 'Codex Alpha', api_type: 'responses' });
const apiSelected = model();
const apiUnselected = model({ id: 'api-unselected', model_id: 'vendor/unselected', selected: false });

const providers = [apiProvider, subscriptionProvider];
const models = [apiSelected, apiUnselected, subscriptionModel];

describe('route candidates', () => {
  it('keeps the all-models scope limited to API models, unlike the pre-fix inline rule', () => {
    // 修复前 App.tsx / Debug 页用的内联规则：只要 all_models 就把订阅模型也算进去。
    const legacyIncludes = (route: RouteRule, candidate: Model) =>
      (route.strategy === 'jev' && route.all_models === true) || route.model_ids.includes(candidate.id);
    expect(legacyIncludes(allModelsRoute, subscriptionModel)).toBe(true);
    expect(routeIncludesModel(allModelsRoute, subscriptionModel, providers)).toBe(false);
    expect(routeIncludesModel(allModelsRoute, apiSelected, providers)).toBe(true);
    // 显式 model_ids 仍然是订阅模型参与路由的唯一途径。
    expect(routeIncludesModel(explicitRoute, subscriptionModel, providers)).toBe(true);
  });

  it('treats every deselected candidate as absent, so an all-models route stops being connectable', () => {
    const allDeselected = [model({ selected: false }), model({ id: 'second', selected: false })];
    expect(routeHasCandidates(allModelsRoute, allDeselected, providers)).toBe(false);
    expect(connectableRoutes([allModelsRoute], allDeselected, providers)).toEqual([]);
    // 只要有一个已选中的 API 模型，路由重新可用。
    expect(connectableRoutes([allModelsRoute], [...allDeselected, apiSelected], providers).map(route => route.id)).toEqual(['everything']);
  });

  it('does not count a subscription-only candidate in an all-models route', () => {
    expect(routeCandidateModels(allModelsRoute, [subscriptionModel], providers)).toEqual([]);
    expect(routeHasCandidates(allModelsRoute, [subscriptionModel], providers)).toBe(false);
    // 显式加入、已选择且启用的订阅模型在前后端都被当作候选（实际生成仍受限准入）。
    expect(routeCandidateModels(explicitRoute, [subscriptionModel], providers).map(candidate => candidate.id)).toEqual(['subscription-selected']);
    expect(routeHasCandidates(explicitRoute, [subscriptionModel], providers)).toBe(true);
  });

  it('excludes disabled, provider-disabled and protocol-incompatible candidates', () => {
    expect(isRouteCandidate(explicitRoute, { ...subscriptionModel, enabled: false }, providers)).toBe(false);
    expect(isRouteCandidate(explicitRoute, subscriptionModel, [{ ...subscriptionProvider, enabled: false }, apiProvider])).toBe(false);
    expect(isRouteCandidate(explicitRoute, { ...subscriptionModel, api_type: 'grpc' }, providers)).toBe(false);
    expect(protocolCompatible({ api_type: '' }, subscriptionProvider)).toBe(true);
    expect(protocolCompatible({ api_type: 'messages' }, apiProvider)).toBe(true);
  });

  it('keeps direct calls by the original model ID out of the candidate rule', () => {
    // Debug／直调的模型清单只要求未停用与服务商启用：取消选择不移出直调。
    const directCallable = models.filter(candidate =>
      candidate.enabled && providers.some(provider => provider.id === candidate.provider_id && provider.enabled));
    expect(directCallable.map(candidate => candidate.id)).toContain('api-unselected');
    expect(isRouteCandidate(allModelsRoute, apiUnselected, providers)).toBe(false);
    // 停用以后直调清单也不再包含它。
    const disabled = models.map(candidate => candidate.id === 'api-unselected' ? { ...candidate, enabled: false } : candidate);
    expect(disabled.filter(candidate =>
      candidate.enabled && providers.some(provider => provider.id === candidate.provider_id && provider.enabled)).map(candidate => candidate.id))
      .not.toContain('api-unselected');
  });
});
