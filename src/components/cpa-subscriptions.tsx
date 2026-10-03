import { useState } from 'react';
import { getSnapshot, createCpaSubscription, cpaSubscriptionAction, selectCpaModel, deleteProvider } from '../lib/bridge';
import { cpaStageLabel, type CpaView } from '../lib/cpa';
import type { DashboardSnapshot } from '../types';

export function CpaSubscriptions({connections,onSnapshot,onNotify}: {
  connections:CpaView[]; onSnapshot:(value:DashboardSnapshot)=>void;
  onNotify:(message:string,error?:boolean)=>void;
}) {
  const [name,setName]=useState('Codex 订阅');
  const [busy,setBusy]=useState<Record<string,string>>({});
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
      <p>目录和登录不授予生成资格。账号、套餐、能力与整次调用仅用订阅内权益的证据需分别验证。</p>
      <p>真实调用：待 R7 验证。当前仅接入 Codex 的隔离授权替身；真实 CPA 服务交付待 R6。</p>
      <form onSubmit={e=> {e.preventDefault();void run('new','create',()=>createCpaSubscription(name));}}>
        <input aria-label="CPA 连接名称" value={name} maxLength={100} onChange={e=>setName(e.target.value)} />
        <button className="button" type="submit" disabled={!name.trim() || !!busy.new}>添加 CPA 连接</button>
      </form>
    </div>
    {connections.map(c=> {
      const pending=c.stage==='starting'||c.stage==='waiting'||busy[c.provider_id]==='begin';
      const action=(verb:Parameters<typeof cpaSubscriptionAction>[1])=>void run(c.provider_id,verb,()=>cpaSubscriptionAction(c.provider_id,verb));
      return <article key={c.provider_id} data-cpa-id={c.provider_id} className="cpa-connection">
        <header><strong>{c.provider} · {c.provider_id}</strong><span data-cpa-stage={c.stage}>{busy[c.provider_id]==='begin' ? '发起授权' : cpaStageLabel[c.stage]}</span></header>
        <p>账号：{c.account??'未知'} · 套餐：{c.plan??'未知'} · 世代：{c.generation}</p>
        <p>{c.catalog_state==='stale'?'历史目录（陈旧）':c.catalog_state==='available'?'目录已发现':'目录未知'}{c.observed_at?` · 最后成功读取 ${c.observed_at}`:''}</p>
        <p>模型资格：未知 · 额度：未知 · 协议能力：未验证</p>
        {!c.service_available && <p role="status">专用 CPA 服务未配置；真实授权未开放。</p>}
        {c.error && <p role="alert">{c.error}</p>}
        {c.authorization_url && <p><a href={c.authorization_url} target="_blank" rel="noreferrer">打开授权页面</a></p>}
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
          <span>{m.name} · {m.model_id} · {m.bound?'已绑定当前身份':'旧目标/尚未绑定'}</span>
          <button className="button" disabled={!!busy[c.provider_id]||c.stage!=='connected'||c.catalog_state!=='available'} onClick={()=>void run(c.provider_id,'select',()=>selectCpaModel(c.provider_id,m.id,true))}>{m.bound?(m.selected?'已选模型':'选择模型'):'重新绑定当前账号'}</button>
          {m.selected&&<button className="button" onClick={()=>void run(c.provider_id,'unselect',()=>selectCpaModel(c.provider_id,m.id,false))}>取消选择</button>}
        </div>)}
      </article>;
    })}
  </section>;
}
