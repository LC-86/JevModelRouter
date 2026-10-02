import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { GrokReadOnlyStatus } from './grok-readonly-status';

const t = (message: string) => message;

describe('GrokReadOnlyStatus', () => {
  it('shows the closed source gate, unknown account evidence, and generation off', () => {
    const markup = renderToStaticMarkup(createElement(GrokReadOnlyStatus, { t }));
    expect(markup).toContain('Source/version verification required');
    expect(markup).toContain('No Grok ACP request was sent');
    expect((markup.match(/>Unknown<\/dd>/g) ?? []).length).toBe(5);
    expect(markup).toContain('data-testid="grok-readonly-generation">Off</dd>');
  });
});
