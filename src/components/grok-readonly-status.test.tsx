import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { GrokReadOnlyStatus } from './grok-readonly-status';

const t = (message: string) => message;

describe('GrokReadOnlyStatus', () => {
  it('shows manual observation, Unknown values before refresh, and generation off', () => {
    const markup = renderToStaticMarkup(createElement(GrokReadOnlyStatus, { t }));
    expect(markup).toContain('Refresh models and usage');
    expect(markup).toContain('Real generation stays off');
    expect((markup.match(/>Unknown<\/dd>/g) ?? []).length).toBeGreaterThanOrEqual(5);
    expect(markup).toContain('data-testid="grok-readonly-generation">Off</dd>');
  });
});
