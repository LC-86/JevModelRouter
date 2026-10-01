import { useState } from 'react';
import { Copy, Plus, Settings2, Trash2, X } from 'lucide-react';
import type { DashboardSnapshot, RouteRule } from '../types';
import { saveRoute, deleteRoute } from '../lib/bridge';
import { usePreferences } from '../lib/preferences-context';
import { isRouteCandidate, routeCandidateModels } from '../lib/route-candidates';
import { Select } from './select';

const strategies = { round_robin: 'Load balancing', jev: 'Intelligent selection' } as const;
const preferences = { balanced: 'Balance quality, cost and speed', cost: 'Lower cost', quality: 'Higher quality', speed: 'Lower latency' } as const;

export function RoutesPage({ snapshot, onChange }: { snapshot: DashboardSnapshot; onChange: (s: DashboardSnapshot) => void }) {
  const { t } = usePreferences();
  const [draft, setDraft] = useState<RouteRule | null>(null);
  const [originalId, setOriginalId] = useState<string>();
  const [creating, setCreating] = useState(false);
  const [remove, setRemove] = useState<RouteRule | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [copied, setCopied] = useState('');
  const act = async (work: () => Promise<DashboardSnapshot>, close = false) => {
    setBusy(true); setError('');
    try { onChange(await work()); if (close) { setDraft(null); setRemove(null); } }
    catch (e) { setError(t(String(e instanceof Error ? e.message : e))); }
    finally { setBusy(false); }
  };
  const speedMode = draft?.strategy === 'jev' && (draft.automatic_policy ?? snapshot.policy).decision_preference === 'speed';
  const allModels = draft?.strategy === 'jev' && draft.all_models === true;
  const candidates = snapshot.models.filter(m => m.enabled && snapshot.providers.some(p => p.id === m.provider_id && p.enabled));
  return <div className="stack lg">
    <div className="page-intro"><div><h2>{t('Routes')}</h2><p>{t('Create reusable routes and bind your agents to a route ID.')}</p></div><button className="button primary" onClick={() => { setError(''); setCreating(true); setOriginalId(undefined); setDraft({ id: '', name: '', strategy: 'jev', all_models: true, model_ids: [], enabled: true }); }}><Plus size={16}/>{t('Add route')}</button></div>
    {error && !draft && !remove && <p role="alert" className="route-error">{error}</p>}
    {copied && <p role="status">{t('Copied')}: {copied}</p>}
    <div className="table-panel provider-table-panel"><table className="provider-table routes-table"><thead><tr>{['Name', 'Routing method', 'Candidate models', 'Enabled status', 'Actions'].map(label => <th key={label} className={label === 'Actions' ? 'provider-actions-heading' : undefined}>{t(label)}</th>)}</tr></thead><tbody>
      {snapshot.routes.map(route => <tr key={route.id}>
        <td><div className="provider-table-name"><div><div className="route-name"><strong>{route.name.trim() || route.id}</strong><button className="icon-action" title={t('Copy route ID')} aria-label={t('Copy route ID')} onClick={async () => { try { await navigator.clipboard.writeText('autojev/' + route.id); setCopied('autojev/' + route.id); } catch { setError(t('Could not copy route ID')); } }}><Copy size={15}/></button></div><small>{route.id}</small></div></div></td>
        <td>{t(strategies[route.strategy === 'fixed' ? 'round_robin' : route.strategy])}{route.strategy === 'jev' && <> · {t(preferences[(route.automatic_policy ?? snapshot.policy).decision_preference || 'balanced'])}</>}</td>
        <td>{(() => {
          if (route.strategy === 'jev' && route.all_models) return <span>{t('All enabled models')} · {routeCandidateModels(route, snapshot.models, snapshot.providers).length}</span>;
          const models = route.model_ids.filter(id => snapshot.models.some(m => m.id === id)).map(id => {
            const model = snapshot.models.find(m => m.id === id);
            const provider = snapshot.providers.find(p => p.id === model?.provider_id);
            const name = model ? model.name || model.model_id : t('Model removed');
            const detail = model ? `${provider?.name ?? '—'} · ${model.model_id}${!model.enabled || !provider?.enabled ? ' · ' + t('Disabled') : !isRouteCandidate(route, model, snapshot.providers) ? ' · ' + t('Not selected') : ''}` : `${name} · ${id}`;
            return { name, detail };
          });
          return <div className="route-candidates" title={models.map(m => m.detail).join('\n')} tabIndex={models.length ? 0 : undefined} aria-label={models.map(m => m.detail).join(', ')}>
            <span className="route-candidate-names">{models.slice(0, 2).map(m => m.name).join(', ') || '—'}</span>
            {models.length > 2 && <span className="route-candidate-count">+{models.length - 2}</span>}
          </div>;
        })()}</td>
        <td><div className="provider-enabled-cell"><button type="button" role="switch" aria-checked={route.enabled} aria-label={t('Enable {provider}', { provider: route.name || route.id })} className={'switch' + (route.enabled ? ' on' : '')} disabled={busy} onClick={() => void act(() => saveRoute({ ...route, enabled: !route.enabled }))}><span/></button><span>{t(route.enabled ? 'ENABLED' : 'DISABLED')}</span></div></td>
        <td><div className="row-actions"><button className="icon-action" aria-label={t('Edit route')} title={t('Edit route')} onClick={() => { setError(''); setCreating(false); setOriginalId(route.id); setDraft({ ...route, strategy: route.strategy === 'fixed' ? 'round_robin' : route.strategy, model_ids: route.model_ids.filter(id => snapshot.models.some(m => m.id === id)) }); }}><Settings2 size={15}/></button><button className="icon-action danger" title={t('Delete route')} aria-label={t('Delete route')} onClick={() => { setError(''); setRemove(route); }}><Trash2 size={15}/></button></div></td>
      </tr>)}
      {!snapshot.routes.length && <tr><td colSpan={5} className="route-empty">{t('No routes yet. Add a route and choose its candidate models.')}</td></tr>}
    </tbody></table></div>
    {draft && <div className="modal-backdrop" onMouseDown={e => { if (e.target === e.currentTarget && !busy) setDraft(null); }}>
      <div className="dialog provider-dialog" role="dialog" aria-modal="true" aria-labelledby="route-title">
        <div className="provider-dialog-header"><div><h2 id="route-title">{t(creating ? 'Add route' : 'Edit route')}</h2><p>{t('Agents request autojev/ followed by this route ID.')}</p></div><button disabled={busy} className="icon-action" aria-label={t('Close')} onClick={() => setDraft(null)}><X size={20}/></button></div>
        <form className="provider-dialog-form route-dialog-form" autoComplete="off" onSubmit={e => { e.preventDefault(); void act(() => saveRoute({ ...draft, all_models: allModels, model_ids: allModels ? [] : draft.model_ids, automatic_policy: draft.strategy === 'jev' ? { ...(draft.automatic_policy ?? snapshot.policy), mode: 'auto' } : draft.automatic_policy, name: draft.name.trim() || draft.id, model_settings: draft.strategy === 'jev' ? {} : Object.fromEntries(Object.entries(draft.model_settings ?? {}).filter(([id]) => draft.model_ids.includes(id))) }, creating, undefined, originalId), true); }}>
          <div className="field-pair"><label className="form-field">{t('Route ID')}<input required pattern="[a-z0-9_\-]{1,64}" disabled={busy} value={draft.id} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} placeholder="auto" onChange={e => setDraft({ ...draft, id: e.target.value })}/></label><label className="form-field">{t('Display name')}<input value={draft.name} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} onChange={e => setDraft({ ...draft, name: e.target.value })}/></label></div>
          <label className="form-field">{t('Routing method')}<Select aria-label={t('Routing method')} searchable={false} disabled={busy} value={draft.strategy} onChange={e => {
            setDraft({ ...draft, strategy: e.target.value as 'round_robin' | 'jev', all_models: e.target.value === 'jev' ? (draft.all_models ?? true) : false });
          }}><option value="round_robin">{t('Load balancing')}</option><option value="jev">{t('Intelligent selection')}</option></Select></label>
          <p>{t(speedMode ? 'Speed priority selects locally using recent measurements, without calling Jev. Test models in the model list. Without fresh comparable data, requests use equal load balancing.' : draft.strategy === 'jev' && (draft.automatic_policy ?? snapshot.policy).decision_preference === 'cost' ? 'Cost priority compares estimated costs among capable candidates. Unknown prices are fallback only.' : draft.strategy === 'jev' ? 'Calls Jev to choose among candidates. Configure the decision service in Settings. If unavailable, automatically falls back to load balancing.' : draft.strategy === 'round_robin' ? 'New sessions are distributed by priority and weight. Requests in the same session keep their model.' : 'All requests use the selected model.')}</p>
          {draft.strategy === 'jev' && <fieldset className="route-decision-settings">
            <legend>{t('Decision settings')}</legend>
            <label className="form-field">{t('Decision preference')}<Select aria-label={t('Decision preference')} searchable={false} disabled={busy} value={(draft.automatic_policy ?? snapshot.policy).decision_preference || 'balanced'} onChange={e => {
              setDraft({ ...draft, automatic_policy: { ...(draft.automatic_policy ?? snapshot.policy), mode: 'auto', decision_preference: e.target.value as 'balanced' | 'cost' | 'quality' | 'speed' } });
            }}><option value="balanced">{t('Balance quality, cost and speed')}</option><option value="cost">{t('Lower cost')}</option><option value="quality">{t('Higher quality')}</option><option value="speed">{t('Lower latency')}</option></Select></label>
            {!speedMode && <><label className="provider-enabled"><input autoComplete="off" autoCapitalize="none" type="checkbox" checked={(draft.automatic_policy ?? snapshot.policy).prefer_local} onChange={e => setDraft({ ...draft, automatic_policy: { ...(draft.automatic_policy ?? snapshot.policy), mode: 'auto', prefer_local: e.target.checked } })}/>{t('Prefer local models')}</label>
            <p className="provider-test-help">{t('Choose Ollama when capability and quality tiers are equivalent.')}</p></>}
            <label className="form-field"><span>{t('Cost baseline')}</span><Select value={(draft.automatic_policy ?? snapshot.policy).savings_baseline_model_id ?? ''} onChange={e => setDraft({ ...draft, automatic_policy: { ...(draft.automatic_policy ?? snapshot.policy), mode: 'auto', savings_baseline_model_id: e.target.value || null } })}><option value="">{t('No baseline')}</option>{snapshot.models.map(m => <option key={m.id} value={m.id}>{m.name}</option>)}</Select></label>
            <p className="provider-test-help">{t('Used to estimate savings by comparing this model’s estimated cost with the selected model’s estimated cost for the same request. It does not affect model selection or billing. Savings are shown as 0 when no baseline is set, the baseline is unavailable for comparison, or the selected model costs more.')}</p>
          </fieldset>}
          {draft.strategy === 'jev' && <label className="form-field">{t('Model scope')}<Select aria-label={t('Model scope')} searchable={false} disabled={busy} value={allModels ? 'all' : 'selected'} onChange={e => setDraft({ ...draft, all_models: e.target.value === 'all' })}><option value="all">{t('All enabled models')}</option><option value="selected">{t('Selected models')}</option></Select></label>}
          {allModels && <p className="provider-test-help">{t('Newly added or enabled models participate automatically. Only available models are considered.')}</p>}
          {!allModels && <section className="route-model-rows">
            <div className="route-model-heading"><strong>{t('Models')}</strong><div className="route-actions"><button type="button" className="button ghost" disabled={busy || !candidates.some(m => !draft.model_ids.includes(m.id))} onClick={() => {
              const ids = [...new Set([...draft.model_ids.filter(Boolean), ...candidates.map(m => m.id)])];
              setDraft({ ...draft, model_ids: ids, model_settings: Object.fromEntries(ids.map(id => [id, draft.model_settings?.[id] ?? { priority: 0, weight: 1 }])) });
            }}>{t('Select all models')}</button><button type="button" className="button ghost" disabled={busy || draft.model_ids.includes('') || draft.model_ids.length >= candidates.length} onClick={() => setDraft({ ...draft, model_ids: [...draft.model_ids, ''] })}><Plus size={16}/>{t('Add model')}</button></div></div>
            {draft.model_ids.map((id, index) => <div className={"route-model-row" + (draft.strategy === 'jev' ? '' : ' balanced')} key={index}>
              <label className="form-field">{t('Model')}<Select disabled={busy} value={id} onChange={e => {
                const next = e.target.value;
                const settings = { ...draft.model_settings };
                settings[next] = settings[id] ?? { priority: 0, weight: 1 };
                delete settings[id];
                setDraft({ ...draft, model_ids: draft.model_ids.map((value, i) => i === index ? next : value), model_settings: settings });
              }}><option value="" disabled>{t('Select model')}</option>{candidates.filter(m => m.id === id || !draft.model_ids.includes(m.id)).map(m => <option key={m.id} value={m.id}>{`${m.provider_id}/${m.model_id}`}</option>)}{id && !candidates.some(m => m.id === id) && snapshot.models.filter(m => m.id === id).map(m => <option key={m.id} value={id}>{`${m.provider_id}/${m.model_id}`} · {t('Disabled')}</option>)}</Select></label>
              {draft.strategy !== 'jev' && <>
                <label className="form-field">{t('Priority')}<input autoComplete="off" autoCapitalize="none" type="number" min="0" max="1000000" step="1" required disabled={busy || !id} value={draft.model_settings?.[id]?.priority ?? 0} onChange={e => setDraft({ ...draft, model_settings: { ...draft.model_settings, [id]: { weight: draft.model_settings?.[id]?.weight ?? 1, priority: e.target.valueAsNumber } } })}/></label>
                <label className="form-field">{t('Weight')}<input autoComplete="off" autoCapitalize="none" type="number" min="1" max="10000" step="1" required disabled={busy || !id} value={draft.model_settings?.[id]?.weight ?? 1} onChange={e => setDraft({ ...draft, model_settings: { ...draft.model_settings, [id]: { priority: draft.model_settings?.[id]?.priority ?? 0, weight: e.target.valueAsNumber } } })}/></label>
              </>}
              <button type="button" className="icon-action" disabled={busy} aria-label={t('Remove model')} onClick={() => { const settings = { ...draft.model_settings }; delete settings[id]; setDraft({ ...draft, model_ids: draft.model_ids.filter((_, i) => i !== index), model_settings: settings }); }}><X size={18}/></button>
            </div>)}
            {!candidates.length && <p>{t('Add an enabled model and provider first.')}</p>}
            {draft.strategy !== 'jev' && <p>{t('Higher priority numbers go first. Equal priorities share traffic by weight. Only confirmed failures before generation starts try remaining candidates.')}</p>}
          </section>}
          <p>{t('Prices follow the selected models. Only enabled models matching the request protocol are eligible.')}</p>
          {error && <p role="alert" className="route-error">{error}</p>}
          <div className="provider-dialog-actions"><button type="button" className="button ghost" disabled={busy} onClick={() => setDraft(null)}>{t('Cancel')}</button><button className="button primary" disabled={busy || (!allModels && (!draft.model_ids.length || draft.model_ids.some(id => !candidates.some(m => m.id === id))))}>{t(busy ? 'Saving…' : 'Save route')}</button></div>
        </form>
      </div>
    </div>}
    {remove && <div className="modal-backdrop"><div className="dialog provider-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-route-title"><div className="provider-dialog-header"><h2 id="delete-route-title">{t('Delete route')} · {remove.name || remove.id}</h2></div><div className="provider-dialog-form"><p>{t('Agents using this route will stop working until another route is selected.')}</p>{error && <p role="alert">{error}</p>}<div className="provider-dialog-actions"><button autoFocus className="button ghost" disabled={busy} onClick={() => setRemove(null)}>{t('Cancel')}</button><button className="button destructive" disabled={busy} onClick={() => void act(() => deleteRoute(remove.id), true)}>{t('Delete route')}</button></div></div></div></div>}
  </div>;
}
