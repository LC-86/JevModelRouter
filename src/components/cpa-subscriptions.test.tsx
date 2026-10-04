import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { CpaSubscriptions } from './cpa-subscriptions';

it('offers an explicit owned service recovery before authorization in a normal desktop', () => {
  const markup=renderToStaticMarkup(createElement(CpaSubscriptions,{connections:[{
    provider_id:'cpa-recovery',provider:'codex',connection_name:'Recover',stage:'idle',connection_instance_id:'instance',generation:1,
    account:null,plan:null,catalog_state:'unknown',observed_at:null,models:[],error:null,authorization_url:null,
    service_available:false,qualification:'unknown',quota:'unknown',capability:'unverified',
    owned_service:{running:false,pid:0,port:19528,binary:'/owned/bin/cpa',artifact_sha256:'pinned'}
  }],onSnapshot:()=>{},onNotify:()=>{}}));
  expect(markup).toContain('启动专用 CPA 服务');
  expect(markup).toContain('/owned/bin/cpa');
  expect(markup).toContain('19528');
  expect(markup).toMatch(/disabled=""[^>]*>发起授权/);
});

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

it('offers distinct Codex and Grok CPA connections with reviewed finite enablement',()=>{
 const markup=renderToStaticMarkup(createElement(CpaSubscriptions,{connections:[],onSnapshot:()=>{},onNotify:()=>{}}));
 expect(markup).toContain('Grok / xAI');
 expect(markup).toContain('逐来源审阅证据后启用');
 expect(markup).toContain('未知费用不是 0');
});
