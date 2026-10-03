import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { CpaSubscriptions } from './cpa-subscriptions';

it('shows the saved connection name beside its stable identity and keeps discovery distinct from verification', () => {
  const markup = renderToStaticMarkup(createElement(CpaSubscriptions, {
    connections: [{ provider_id:'cpa-a',provider:'codex',connection_name:'团队 Codex',stage:'connected',connection_instance_id:'instance-a',generation:3,account:'fictional-a',plan:null,catalog_state:'stale',observed_at:'2026-10-03T00:00:00Z',models:[{id:'model-a',model_id:'same-model',name:'Same model',selected:true,bound:false}],error:null,authorization_url:null,service_available:true,qualification:'unknown',quota:'unknown',capability:'unverified' }],
    onSnapshot: () => {}, onNotify: () => {},
  }));
  expect(markup).toContain('套餐：未知');
  expect(markup).toContain('历史目录（陈旧）');
  expect(markup).toContain('团队 Codex · codex · cpa-a');
  expect(markup).toContain('连接实例：instance-a · 世代：3');
  expect(markup).toContain('autojev/model/model-a');
  expect(markup).toContain('真实调用：待 R7 验证');
  expect(markup).toContain('目录和登录不授予生成资格');
  expect(markup).not.toContain('API key');
});
