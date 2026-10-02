export interface ObservedField<T> { state: string; value: T | null }
export interface GrokObservation {
  observedAt: string;
  models: ObservedField<string[]>;
  currentModel: ObservedField<string>;
  usagePercent: ObservedField<number>;
  remainingPercent: ObservedField<number>;
  subscriptionTier: ObservedField<string>;
  periodType: ObservedField<string>;
  periodStart: ObservedField<string>;
  periodEnd: ObservedField<string>;
  billingPeriodEnd: ObservedField<string>;
  periodConflict: boolean;
  realGenerationEnabled: false;
}
export const GROK_OBSERVATION_TTL_MS = 5 * 60 * 1000;
export function observationExpired(value: GrokObservation, now = Date.now()): boolean {
  const observed = Date.parse(value.observedAt);
  return !Number.isFinite(observed) || now < observed || now - observed >= GROK_OBSERVATION_TTL_MS;
}
/** Only the still-current manual request may publish. Cancel/unmount/next refresh invalidates its ID. */
export function acceptGrokObservation(requestId: string, currentId: string | null, value: GrokObservation): GrokObservation | null {
  return requestId === currentId ? value : null;
}
export function fieldText<T>(field: ObservedField<T> | undefined, format: (value: T) => string, unknown: string): string {
  if (!field || field.state !== 'available' || field.value === null) return field ? `${unknown} (${field.state})` : unknown;
  return format(field.value);
}
