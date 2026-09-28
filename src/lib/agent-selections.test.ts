import { expect, it } from 'vitest';
import { availableAgentSelections, parseAgentSelections } from './agent-selections';

it('restores per-agent selections, default order and explicitly cleared selections', () => {
  const saved = { codex: ['fast', 'model/a', 'cheap'], claude: [] };
  expect(parseAgentSelections(JSON.stringify(saved))).toEqual(saved);
});
it('ignores malformed entries and deduplicates without changing the default', () => {
  expect(parseAgentSelections('{broken')).toEqual({});
  expect(parseAgentSelections('[]')).toEqual({});
  expect(parseAgentSelections(JSON.stringify({ codex: ['fast', 'fast', 'cheap'], claude: [null], hermes: [] })))
    .toEqual({ codex: ['fast', 'cheap'], hermes: [] });
});

it('excludes unavailable bindings while preserving default order and empty selections', () => {
  const available = new Set(['fast', 'model/b', 'quality']);
  expect(availableAgentSelections(['model/disabled', 'fast', 'model/deleted', 'model/b', 'fast'], available)).toEqual(['fast', 'model/b']);
  expect(availableAgentSelections(['model/disabled'], available)).toEqual([]);
  expect(availableAgentSelections([], available)).toEqual([]);
});
