import type { Model, Provider, RouteRule } from '../types';
import { isSubscriptionProvider, modelSelected } from './subscription';

/**
 * 「上游协议」判定：与后端 `Protocol::upstream` 一致——模型自带 `api_type` 优先，否则看服务商；
 * 空值按 `chat_completions` 处理，只有无法识别的值才算不兼容。
 */
const UPSTREAM_PROTOCOLS = new Set(['', 'chat_completions', 'chat/completions', 'responses', 'messages']);

export function upstreamProtocol(model: Pick<Model, 'api_type'>, provider: Provider | undefined): string | null {
  const value = model.api_type && model.api_type.length > 0 ? model.api_type : provider?.api_type ?? '';
  return UPSTREAM_PROTOCOLS.has(value) ? value : null;
}

export function protocolCompatible(model: Pick<Model, 'api_type'>, provider: Provider | undefined): boolean {
  return provider !== undefined && upstreamProtocol(model, provider) !== null;
}

type RouteScope = Pick<RouteRule, 'strategy' | 'all_models' | 'model_ids'>;

/**
 * 路由作用域：显式 `model_ids` 一律包含；`all_models` 只自动包含 API 模型。
 * 与后端 `router::rule_includes_model` 一致——订阅模型只能通过显式 `model_ids` 加入。
 */
export function routeIncludesModel(route: RouteScope, model: Pick<Model, 'id' | 'provider_id'>, providers: Provider[]): boolean {
  if (route.model_ids.includes(model.id)) return true;
  if (!(route.strategy === 'jev' && route.all_models === true)) return false;
  return !isSubscriptionProvider(providers.find(provider => provider.id === model.provider_id));
}

/**
 * 一条路由的完整候选判定，与后端 `router::route_candidate` 同一套条件：
 * 作用域 + 用户选择 + 停用 + 服务商启用 + 协议兼容。
 *
 * 账号资格、能力与额度属于派发准入，不在这里判定（显式加入的订阅候选可以连接，
 * 真正生成仍要过后端准入）。取消选择只影响候选与列表；按原模型 ID 直调不经过这里。
 */
export function isRouteCandidate(route: RouteScope, model: Model, providers: Provider[]): boolean {
  const provider = providers.find(candidate => candidate.id === model.provider_id);
  return modelSelected(model)
    && model.enabled
    && provider?.enabled === true
    && routeIncludesModel(route, model, providers)
    && protocolCompatible(model, provider);
}

export function routeCandidateModels(route: RouteScope, models: Model[], providers: Provider[]): Model[] {
  return models.filter(model => isRouteCandidate(route, model, providers));
}

export function routeHasCandidates(route: RouteScope, models: Model[], providers: Provider[]): boolean {
  return models.some(model => isRouteCandidate(route, model, providers));
}

/** 界面可用性：只有已启用且至少有一个当前候选的路由才算可选，与后端连接校验一致。 */
export function connectableRoutes(routes: RouteRule[], models: Model[], providers: Provider[]): RouteRule[] {
  return routes.filter(route => route.enabled && routeHasCandidates(route, models, providers));
}
