import { useState } from 'react';
import { getSnapshot, setCodingPlanHandRun } from '../lib/bridge';
import type { DashboardSnapshot } from '../types';

export interface CodingPlanView {
  provider_id:string;name:string;endpoint:string;connection_instance_id:string;generation:number;
  credential_reference:string;account:string|null;plan:string|null;evidence_file:string;artifact_sha256:string;
  models:{id:string;model_id:string;selected:boolean}[];
  hand_run:{enabled:boolean;stopped:boolean;used:number;max_requests:number;expires_at:number;model_id:string}|null;
}
export function CodingPlanHandRuns({connections,onSnapshot,onNotify}:{connections:CodingPlanView[];onSnapshot:(s:DashboardSnapshot)=>void;onNotify:(s:string,error?:boolean)=>void}) {
  const [busy,setBusy]=useState<string|null>(null);
  const run=async(id:string,enabled:boolean)=>{
    setBusy(id);
    try{onSnapshot(await setCodingPlanHandRun(id,enabled));}
    catch(e){onNotify(String(e),true);try{onSnapshot(await getSnapshot());}catch{/* Keep the last view. */}}
    finally{setBusy(null);}
  };
  if(!connections.length)return null;
  return <section className="table-panel" aria-label="Coding Plan 有限验证" data-testid="coding-plan-hand-runs">
    <strong>Coding Plan Key</strong>
    <p>逐来源审阅身份、套餐、模型资格、协议、额度和整次无额外消费证据后启用有限计划。未知费用不是 0。失败即停；重开默认关闭。</p>
    {connections.map(c=><article key={c.provider_id} data-coding-plan-id={c.provider_id}>
      <strong>{c.name} · {c.provider_id}</strong>
      <p>专用端点：{c.endpoint} · 实例：{c.connection_instance_id} · 世代：{c.generation}</p>
      <p>用户声明账号：{c.account??'未知'} · 用户声明套餐：{c.plan??'未知'} · 凭据引用：{c.credential_reference}</p>
      <p>身份、套餐、资格、协议、额度及整次费用：{c.hand_run?.enabled?'已审临时证据':'未审查 / 权限关闭'}</p>
      <p>当前产物：<code>{c.artifact_sha256}</code> · 审阅后证据文件：<code>{c.evidence_file}</code></p>
      {c.models.map(m=><p key={m.id}>上游模型：{m.model_id} · 固定调用 ID：autojev/model/{m.id} · {m.selected?'已选':'未选'}</p>)}
      <p>生成：{c.hand_run?.enabled?'有限计划已启用':'关闭'} · 请求 {c.hand_run?.used??0}/{c.hand_run?.max_requests??0}{c.hand_run?.stopped?' · 失败锁定':''}</p>
      <button className="button" disabled={busy!==null||!c.models.some(m=>m.selected)} onClick={()=>void run(c.provider_id,true)}>启用此 Coding Plan 有限 HAND_RUN</button>
      <button className="button" disabled={busy!==null||!c.hand_run?.enabled} onClick={()=>void run(c.provider_id,false)}>关闭此 Coding Plan 生成</button>
    </article>)}
  </section>;
}
