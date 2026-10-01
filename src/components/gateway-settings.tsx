import { CircleCheck, CircleAlert, LoaderCircle, Play } from 'lucide-react';
import { SpeedTestSettings } from './model-speed-tests';
import { useState } from 'react';
import type { DashboardSnapshot, DecisionProvider, GatewaySettings } from '../types';
import { gatewayDefaults, saveGatewaySettings, savePolicy, testJevSettings } from '../lib/bridge';
import { usePreferences } from '../lib/preferences-context';
import { Select } from './select';

const decisionDefaults: Record<DecisionProvider, { jev_endpoint: string; jev_model: string }> = {
  openrouter: { jev_endpoint: 'https://openrouter.ai/api/alpha/decisions', jev_model: '~typesafe/jev-latest' },
  zenmux: { jev_endpoint: 'https://zenmux.ai/api/v1/systemone', jev_model: 'typesafe/jev-1.13' },
};

export function GatewaySettingsPanel({ snapshot, onChange, onRefreshBudgetSnapshot }: { snapshot: DashboardSnapshot; onChange: (s: DashboardSnapshot) => void; onRefreshBudgetSnapshot: () => Promise<DashboardSnapshot> }) {
  const { t } = usePreferences();
  const [gateway, setGateway] = useState(snapshot.gateway ?? gatewayDefaults);
  const [tab, setTab] = useState('jev');
  const tabs = [['jev', 'Decision model'], ['gateway', 'Gateway settings'], ['speed', 'Model speed tests']] as const;
  const [policy, setPolicy] = useState(snapshot.policy);
  const [key, setKey] = useState('');
  const needsNewKey = policy.decision_provider !== snapshot.policy.decision_provider && !key.trim();
  const [busy, setBusy] = useState(false);
  const [testing, setTesting] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const act = async (work: () => Promise<DashboardSnapshot>) => {
    setBusy(true); setMessage(''); setError('');
    try { onChange(await work()); setMessage(t('Saved')); }
    catch (e) { setError(String(e instanceof Error ? e.message : e)); }
    finally { setBusy(false); }
  };
  const fields: [keyof GatewaySettings, string, number, number][] = [
    ['response_timeout_seconds', 'Response timeout (seconds)', 1, 600],
    ['stream_idle_seconds', 'Stream idle timeout (seconds)', 1, 1800],
    ['max_attempts', 'Maximum attempts per request', 1, 16],
    ['failure_threshold', 'Failures before cooldown', 1, 20],
    ['cooldown_seconds', 'Cooldown (seconds)', 1, 3600],
  ];
  return <div className="stack gateway-settings">
    <div className="gateway-tabs" role="tablist" aria-label={t('Gateway settings')}>
      {tabs.map(([id, label], index) => <button key={id} type="button" role="tab" id={`gateway-tab-${id}`}
        aria-controls={`gateway-panel-${id}`} aria-selected={tab === id} tabIndex={tab === id ? 0 : -1}
        disabled={busy} onClick={() => { setTab(id); setError(''); setMessage(''); }}
        onKeyDown={e => {
          const next = e.key === 'ArrowRight' ? (index + 1) % tabs.length : e.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : e.key === 'Home' ? 0 : e.key === 'End' ? tabs.length - 1 : -1;
          if (next < 0) return;
          e.preventDefault(); setTab(tabs[next][0]); setError(''); setMessage('');
          document.getElementById(`gateway-tab-${tabs[next][0]}`)?.focus();
        }}>{t(label)}</button>)}
    </div>
    <div role="tabpanel" id={`gateway-panel-${tab}`} aria-labelledby={`gateway-tab-${tab}`} className="stack">
    {tab === 'speed' && <SpeedTestSettings onBudgetSnapshotRefresh={onRefreshBudgetSnapshot}/>}
    {tab === 'jev' && <>
    <form className="settings-group stack decision-model-form" onChange={() => { setError(''); setMessage(''); }} onSubmit={e => { e.preventDefault(); void act(async () => { const result = await savePolicy(policy, key || undefined); setKey(''); return result; }); }}>
      <div className="form-field"><label htmlFor="decision-provider">{t('Decision provider')}</label><Select id="decision-provider" aria-label={t('Decision provider')} searchable={false} disabled={busy} value={policy.decision_provider} onChange={e => {
        const selected = e.target.value as DecisionProvider;
        if (selected === policy.decision_provider) return;
        setPolicy({ ...policy, decision_provider: selected, ...decisionDefaults[selected] });
        setKey('');
      }}><option value="openrouter">OpenRouter</option><option value="zenmux">ZenMux</option></Select></div>
      <label className="form-field">{t('Decision key')}<input autoCapitalize="none" autoCorrect="off" spellCheck={false} disabled={busy} type="password" autoComplete="off" value={key} placeholder={snapshot.policy.has_autojev_key && !needsNewKey ? t('Credential stored') : 'sk-…'} onChange={e => setKey(e.target.value)} /></label>
      <p className="settings-description">{t(needsNewKey ? 'Enter a new decision API key after changing provider' : 'Leave blank to keep the stored key.')}</p>
      <label className="form-field">{t('Decision model ID')}<input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} disabled={busy} required value={policy.jev_model ?? ''} placeholder={decisionDefaults[policy.decision_provider].jev_model} onChange={e => setPolicy({ ...policy, jev_model: e.target.value })}/></label>
      <p className="settings-description">{t('Used to select candidate models; only routing metadata is sent, not conversation content.')}</p>
      <div className="provider-dialog-actions"><button type="button" className="button ghost" disabled={busy || !policy.jev_model?.trim() || needsNewKey} onClick={async () => {
        setBusy(true);setTesting(true);setError('');setMessage('');try{const model=await testJevSettings(policy,key);setMessage(t('Decision model test passed. Selected: {model}',{model}));}catch(e){setError(t(String(e instanceof Error ? e.message : e)));}finally{setBusy(false);setTesting(false);}
      }}>{testing ? <LoaderCircle size={15} className="import-spinner"/> : <Play size={15}/>} {t(testing ? 'Testing…' : 'Test')}</button><button className="button primary" disabled={busy || needsNewKey}>{t('Save decision model')}</button></div>
    </form>
    </>}
    {tab === 'gateway' && <form className="settings-group gateway-form" onSubmit={e => { e.preventDefault(); void act(() => saveGatewaySettings(gateway)); }}>
      <section className="gateway-form-section"><h4>{t('Failover and timeouts')}</h4>
      <p className="settings-description">{t('Retries connection errors, 401, 402, 403, 408, 429 and 5xx before output starts. Each candidate is tried at most once. Rate limits respect Retry-After; recovery allows one probe at a time.')}</p>
      <div className="field-pair">{fields.map(([name, label, min, max]) => <label className="form-field" key={name}>{t(label)}<input autoComplete="off" autoCapitalize="none" type="number" required min={min} max={max} step="1" value={gateway[name]} onChange={e => setGateway({ ...gateway, [name]: e.target.valueAsNumber })}/></label>)}</div>
      </section>
      <section className="gateway-form-section"><h4>{t('Outbound proxy')}</h4>
      <p className="settings-description">{t('Stop the gateway before changing proxy or connection timeout. Connected agents are restored before it stops.')}</p>
      <div className="field-pair"><label className="form-field">{t('Connection timeout (seconds)')}<input autoComplete="off" autoCapitalize="none" type="number" min="1" max="120" required disabled={snapshot.proxy.running} value={gateway.connect_timeout_seconds} onChange={e => setGateway({ ...gateway, connect_timeout_seconds: e.target.valueAsNumber })}/></label>
      <label className="form-field">{t('Proxy mode')}<Select disabled={snapshot.proxy.running} value={gateway.proxy_mode} onChange={e => setGateway({ ...gateway, proxy_mode: e.target.value as GatewaySettings['proxy_mode'] })}><option value="system">{t('System proxy')}</option><option value="direct">{t('Direct connection')}</option><option value="custom">{t('Custom proxy')}</option></Select></label></div>
      {gateway.proxy_mode === 'custom' && <label className="form-field">{t('Proxy URL')}<input autoComplete="off" autoCapitalize="none" disabled={snapshot.proxy.running} required type="url" placeholder="http://127.0.0.1:7890" value={gateway.proxy_url} onChange={e => setGateway({ ...gateway, proxy_url: e.target.value })}/></label>}
      </section>
      <div className="provider-dialog-actions"><button className="button primary" disabled={busy}>{t('Save gateway settings')}</button></div>
    </form>}
    </div>
    {error && <div className="gateway-feedback error" role="alert"><CircleAlert size={16} aria-hidden="true"/><p>{error}</p></div>}
    {message && <div className="gateway-feedback success" role="status"><CircleCheck size={16} aria-hidden="true"/><p>{message}</p></div>}
  </div>;
}
