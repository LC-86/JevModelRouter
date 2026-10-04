import { useEffect, useState } from 'react';
import { getSnapshot, createCpaSubscription, cpaSubscriptionAction, deleteProvider, configureCpaService, stopCpaService, setCpaHandRun, openCpaAuthorization } from '../lib/bridge';
import { cpaStageLabel, type CpaView } from '../lib/cpa';
import type { DashboardSnapshot } from '../types';

function OwnedService({connection:c,busy,run}: {connection:CpaView;busy:boolean;run:(action:string,operation:()=>Promise<DashboardSnapshot>)=>void}) {
  const [binary,setBinary]=useState(c.owned_service?.binary??'');
  const [port,setPort]=useState(c.owned_service?.port??0);
  useEffect(()=>{if(c.owned_service){setPort(c.owned_service.port);setBinary(c.owned_service.binary);}},[c.owned_service?.port,c.owned_service?.binary]);
  return <div data-testid={`cpa-owned-service-${c.provider_id}`}>
    <p>专用自有服务：{c.owned_service?.running?'运行中':'已停止 / 未配置'}。启动不登录、不查询额度、不授予生成资格。</p>
    <label>固定 CPA 文件<input aria-label="专用 CPA 文件" value={binary} onChange={e=>setBinary(e.target.value)}/></label>
    <label>专用端口（首次 0 自动分配）<input aria-label="专用 CPA 端口" type="number" min="0" max="65535" value={port} onChange={e=>setPort(Number(e.target.value))}/></label>
    <button className="button" disabled={busy||!binary||c.owned_service?.running} onClick={()=>run('service',()=>configureCpaService(c.provider_id,binary,port))}>启动专用 CPA 服务</button>
    <button className="button" disabled={busy||!c.owned_service?.running} onClick={()=>run('stop-service',()=>stopCpaService(c.provider_id))}>停止自有 CPA 服务</button>
    {c.owned_service&&<p>恢复须用保存端口 {c.owned_service.port}；端口占用由占用者处理。artifact：{c.owned_service.artifact_sha256}</p>}
  </div>;
}

export function CpaSubscriptions({connections,onSnapshot,onNotify}: {
  connections:CpaView[]; onSnapshot:(value:DashboardSnapshot)=>void;
  onNotify:(message:string,error?:boolean)=>void;
}) {
  const [provider,setProvider]=useState<'codex'|'xai'>('codex');
  const [name,setName]=useState('Codex 订阅');
  const [busy,setBusy]=useState<Record<string,string>>({});
  const ownsService=connections.some(c=>!!c.owned_service);
  useEffect(()=>{
    if(!ownsService)return;
    const timer=setInterval(()=>{if(!document.hidden)void getSnapshot().then(onSnapshot).catch(()=>{});},3000);
    return ()=>clearInterval(timer);
  },[ownsService,onSnapshot]);
  const run=async(id:string,action:string,operation:()=>Promise<DashboardSnapshot>)=> {
    setBusy(v=>({...v,[id]:action}));
    try { onSnapshot(await operation()); }
    catch(error) {
      onNotify(String(error),true);
      try { onSnapshot(await getSnapshot()); } catch { /* Keep the last known view. */ }
    } finally { setBusy(v=> { const next={...v}; if(next[id]===action)delete next[id]; return next; }); }
  };
  return <section className="table-panel cpa-subscriptions" data-testid="cpa-subscriptions" aria-label="CPA 订阅连接">
    <div className="cpa-subscriptions-intro">
      <strong>CPA 订阅连接</strong>
      <p>目录和登录不授予生成资格。账号、套餐、能力与整次调用仅用订阅内权益的证据需分别验证。未知费用不是 0。</p>
      <p>真实调用：待 R7 验证。逐来源审阅证据后启用有限 HAND_RUN；未知身份、套餐、能力、额度或费用均阻止派发。</p>
      <form onSubmit={e=> {e.preventDefault();void run('new','create',()=>createCpaSubscription(name,provider));}}>
<select aria-label="CPA 来源" value={provider} onChange={e=>setProvider(e.target.value as 'codex'|'xai')}><option value="codex">Codex</option><option value="xai">Grok / xAI</option></select>
        <input aria-label="CPA 连接名称" value={name} maxLength={100} onChange={e=>setName(e.target.value)} />
        <button className="button" type="submit" disabled={!name.trim() || !!busy.new}>添加 CPA 连接</button>
      </form>
    </div>
    {connections.map(c=> {
      const pending=c.stage==='starting'||c.stage==='waiting'||busy[c.provider_id]==='begin';
      const action=(verb:Parameters<typeof cpaSubscriptionAction>[1])=>void run(c.provider_id,verb,()=>cpaSubscriptionAction(c.provider_id,verb));
      return <article key={c.provider_id} data-cpa-id={c.provider_id} className="cpa-connection">
        <header><strong>{c.connection_name} · {c.provider} · {c.provider_id}</strong><span data-cpa-stage={c.stage}>{busy[c.provider_id]==='begin' ? '发起授权' : cpaStageLabel[c.stage]}</span></header>
        <p>连接实例：{c.connection_instance_id} · 世代：{c.generation}</p>
        <OwnedService connection={c} busy={!!busy[c.provider_id]||pending} run={(verb,operation)=>void run(c.provider_id,verb,operation)}/>
        <p>账号：{c.account??'未知'} · 套餐：{c.plan??'未知'}</p>
        {c.credential_reference&&<p>自有凭据引用（非原文）：{c.credential_reference}</p>}
        <p>{c.catalog_state==='stale'?'历史目录（陈旧）':c.catalog_state==='available'?'目录已发现':'目录未知'}{c.observed_at?` · 最后成功读取 ${c.observed_at}`:''}</p>
        <p>模型资格：{c.qualification==='reviewed_available'?'已审临时证据':'未知'} · 额度：{c.quota==='reviewed_available'?'已审临时证据':'未知'} · 协议能力：{c.capability==='reviewed_chat'?'已审 Chat 证据':'未验证'}</p>
        {!c.service_available && <p role="status">专用 CPA 服务不可用；恢复后需重新核实并启用有限计划。</p>}
        {c.error && <p role="alert">{c.error}</p>}
        {c.provider==='xai'&&<p>Grok 套餐与整次无额外消费证据尚未建立；当前保持不可调用。compact、WebSocket、媒体和 API 切换均未验证。</p>}
        <div data-testid={`cpa-hand-run-${c.provider_id}`}>
          <p>真实生成：{c.hand_run?.enabled&&!c.hand_run.stopped?'有限计划已启用':'关闭'} · 请求 {c.hand_run?.used??0}/{c.hand_run?.max_requests??0}</p>
          {c.owned_service?.evidence_file&&<p>审阅后证据文件：<code>{c.owned_service.evidence_file}</code></p>}
          <button className="button" disabled={!!busy[c.provider_id]||!c.service_available||c.stage!=='connected'||!c.models.some(m=>m.selected&&m.bound)} onClick={()=>void run(c.provider_id,'enable',()=>setCpaHandRun(c.provider_id,true))}>为此来源启用有限 HAND_RUN</button>
          <button className="button" disabled={!!busy[c.provider_id]||!c.hand_run?.enabled} onClick={()=>void run(c.provider_id,'disable',()=>setCpaHandRun(c.provider_id,false))}>关闭此来源生成</button>
        </div>
        {c.authorization_url && <p><a href={c.authorization_url} target="_blank" rel="noreferrer" onClick={e=>{e.preventDefault();void openCpaAuthorization(c.provider_id).catch(error=>onNotify(String(error),true));}}>打开授权页面</a></p>}
        <div className="provider-actions">
          <button className="button" disabled={!c.service_available||!!busy[c.provider_id]||pending||c.stage==='connected'} onClick={()=>action('begin')}>发起授权</button>
          {pending && <>
            <button className="button" disabled={!!busy[c.provider_id]} onClick={()=>action('poll')}>检查授权结果</button>
            <button className="button" onClick={()=>action('cancel')}>取消授权</button>
          </>}
          <button className="button" disabled={!c.service_available||!!busy[c.provider_id]||c.stage!=='connected'} onClick={()=>action('refresh')}>读取目录</button>
          <button className="button" disabled={!!busy[c.provider_id]} onClick={()=>action('disconnect')}>退出连接</button>
          <button className="button" disabled={!c.service_available||!!busy[c.provider_id]||c.stage!=='connected'} onClick={()=>action('switch')}>更换账号</button>
          <button className="button" disabled={!!busy[c.provider_id]||pending||c.stage==='connected'} onClick={()=>void run(c.provider_id,'remove',()=>deleteProvider(c.provider_id))}>删除连接</button>
        </div>
        {c.models.map(m=><div className="cpa-model" key={m.id} data-cpa-model={m.id}>
          <span>{m.name} · 上游 ID：{m.model_id} · 固定调用 ID：autojev/model/{m.id} · {m.bound?'已绑定当前身份':'旧目标/尚未绑定'} · {m.selected?'已选入模型池':'未选'}</span>
        </div>)}
      </article>;
    })}
  </section>;
}
