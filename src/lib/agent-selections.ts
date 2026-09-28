export const AGENT_SELECTIONS_KEY = 'autojev.agent.selections.v1';

export function parseAgentSelections(raw: string | null): Record<string, string[]> {
  try {
    const value: unknown = JSON.parse(raw ?? '{}');
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
    return Object.fromEntries(Object.entries(value)
      .filter((entry): entry is [string, string[]] => Array.isArray(entry[1]) && entry[1].every(item => typeof item === 'string' && item.length > 0))
      .map(([id, bindings]) => [id, [...new Set(bindings)]]));
  } catch { return {}; }
}

export function availableAgentSelections(bindings: string[], available: ReadonlySet<string>): string[] {
  return [...new Set(bindings)].filter(binding => available.has(binding));
}
