import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { expect, it } from 'vitest';
import { CodingPlanHandRuns } from './coding-plan-hand-runs';

it('keeps declared plan labels separate from reviewed evidence and offers a source-bound finite plan',()=>{
  const markup=renderToStaticMarkup(createElement(CodingPlanHandRuns,{connections:[{
    provider_id:'plan-a',name:'Plan A',endpoint:'https://plan.example.invalid/coding/v4',connection_instance_id:'instance-a',generation:1,
    credential_reference:'source:instance-a',account:'声明账号',plan:'声明套餐',evidence_file:'/owned/hand-runs/instance-a/coding-plan.reviewed.json',
    artifact_sha256:'a'.repeat(64),models:[{id:'fixed-model',model_id:'plan-model',selected:true}],hand_run:null,
  }],onSnapshot:()=>{},onNotify:()=>{}}));
  expect(markup).toContain('声明账号');
  expect(markup).toContain('身份、套餐、资格、协议、额度及整次费用：未审查');
  expect(markup).toContain('/coding/v4');
  expect(markup).toContain('autojev/model/fixed-model');
  expect(markup).toContain('启用此 Coding Plan 有限 HAND_RUN');
  expect(markup).toContain('未知费用不是 0');
  expect(markup).not.toContain('Bearer');
});
