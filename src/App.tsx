import { AddAgentDialog } from './components/add-agent-dialog';
import { GatewayActionMenu } from './components/gateway-action-menu';
import { AGENT_SELECTIONS_KEY, availableAgentSelections, parseAgentSelections } from './lib/agent-selections';
import { SpeedCell, SpeedTestToolbar, useModelSpeedTests, type ModelSpeedTests } from './components/model-speed-tests';
import { listen } from '@tauri-apps/api/event';
import { isTauri } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { DebugPage } from './components/debug-page';
import { WindowFrame } from './components/window-frame';
import { TrafficPage } from './components/traffic-page';
import { RoutesPage } from './components/routes-page';
import { sortProviders } from './lib/provider-sort';
import { ApiSourceFields, API_SOURCE_LABELS } from './components/api-source-fields';
import type { ApiSourceDraft } from './types';
import { providerTestError, type ProviderTestStatus } from './lib/provider-test';
import { providerIdentifier } from './lib/provider-name';
import { Select } from './components/select';
import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  FilePenLine,
  Terminal,
  Image as ImageIcon,
  ImageOff,
  ChartNoAxesCombined,
  ArrowRight,
  Bot,
  Box,
  Check,
  ChevronRight,
  ChevronDown,
  Download,
  LoaderCircle,
  CircleGauge,
  Cpu,
  Database,
  Eye,
  EyeOff,
  GitBranch,
  KeyRound,
  Layers3,
  LogIn,
  LogOut,
  Network,
  Play,
  Plus,
  Power,
  RefreshCw,
  Unplug,
  Plug,
  Save,
  Server,
  Settings2,
  ShieldCheck,
  Sparkles,
  Trash2,
  X,
  Zap,
} from 'lucide-react';
import { useAppUpdate } from './components/app-update';
import { SettingsDialog, type SettingsSection } from './components/settings-dialog';
import { SubscriptionAuthDialog } from './components/subscription-auth-dialog';
import { GrokReadOnlyStatus } from './components/grok-readonly-status';
import { CpaSubscriptions } from './components/cpa-subscriptions';
import { CodingPlanHandRuns } from './components/coding-plan-hand-runs';
import { CpaDevelopmentService } from './components/cpa-development-service';
import { usePreferences } from './lib/preferences-context';
import { SearchSelect } from './components/search-select';
import { BrandMark } from './components/brand-mark';
import { rangeStart, summarize } from './lib/traffic';
import {
  CATALOG_REFERENCE_URL, QUOTA_REFERENCE_URL, agentCatalogPendingSync, agentSelectableModels, availabilityLabel,
  capabilityLabel, catalogLabel, catalogModelRow, catalogRemovedLabel, connectionStateLabel, connectionStateTone,
  creditsAxisText, denialLabel, eligibilityLabel, identityLabel, isSubscriptionKind, isSubscriptionProvider,
  localLogoutLabel, loginStageLabel, loginStageTone, modelCapability, modelListMembers, modelSelected, protocolKey,
  quotaHistoryLabel, quotaLabel, quotaPermissionLabel, quotaViewLabel, remoteRevocationLabel, speedTestCandidates,
  subscriptionActions, subscriptionAuthView, subscriptionCatalog, subscriptionCatalogText, subscriptionQuotaText,
  subscriptionReason, subscriptionStatusText, subscriptionView,
} from './lib/subscription';
import { connectableRoutes } from './lib/route-candidates';
import {
  beginSubscriptionLogin,
  cancelSubscriptionLogin,
  connectAgent,
  deleteModel,
  deleteProvider,
  detectAgents,
  logoutSubscription,
  openAgentConfig,
  launchAgent,
  getSnapshot,
  getRequestLogs,
  getGatewayHealth,
  resetGatewayHealth,
  importProviders,
  refreshSubscription,
  selectCpaModel,
  setCodexRealGenerationEnabled,
  setGrokRealGenerationEnabled,
  restoreAgent,
  saveModel,
  saveProvider,
  switchSubscriptionAccount,
  testProvider,
  testProviderDraft,
  toggleProxy,
  pauseProxy,
} from './lib/bridge';
import type {
  DashboardSnapshot,
  Model,
  Provider,
  SubscriptionCatalogEntry,
} from './types';

type Page = 'overview' | 'providers' | 'models' | 'router' | 'agents' | 'activity' | 'usage' | 'debug';

const EMPTY_PROVIDER: Provider = {
  id: '',
  name: '',
  kind: 'openrouter',
  base_url: 'https://openrouter.ai/api/v1',
  enabled: true,
  has_api_key: false,
};

const EMPTY_MODEL: Model = {
  id: '',
  provider_id: '',
  model_id: '',
  name: '',
  tier: 'balanced',
  enabled: true,
  selected: true,
  supports_tools: true,
  supports_vision: false,
  supports_reasoning: false,
  context_window: 1000000,
  api_type: '',
  input_price_known: false, output_price_known: false, cache_price_known: false,
  cache_cost_per_million: 0,
  input_cost_per_million: 0,
  output_cost_per_million: 0,
};

function money(value: number | null) {
  if (value == null) return '—';
  if (value === 0) return '$0';
  if (value < 0.01) return `$${value.toFixed(4)}`;
  return `$${value.toFixed(2)}`;
}

function timeAgo(value: string, t: (message: string, values: Record<string, number>) => string) {
  const seconds = Math.max(1, Math.floor((Date.now() - new Date(value).getTime()) / 1000));
  if (seconds < 60) return t('{count}s ago', { count: seconds });
  if (seconds < 3600) return t('{count}m ago', { count: Math.floor(seconds / 60) });
  return t('{count}h ago', { count: Math.floor(seconds / 3600) });
}

function cx(...classes: Array<string | false | undefined>) {
  return classes.filter(Boolean).join(' ');
}

export default function App() {
  const { t } = usePreferences();
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [stopGatewayOpen, setStopGatewayOpen] = useState(false);
  const [gatewayBusy, setGatewayBusy] = useState(false);
  const gatewayOperation = useRef(false);
  const gatewayButton = useRef<HTMLButtonElement>(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('general');
  const update = useAppUpdate();
  useEffect(() => {
    const shortcut = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === ',') { event.preventDefault(); setSettingsOpen(true); }
    };
    window.addEventListener('keydown', shortcut);
    return () => window.removeEventListener('keydown', shortcut);
  }, []);
  const [snapshot, setSnapshot] = useState<DashboardSnapshot | null>(null);
  const [page, setPage] = useState<Page>('overview');
  const refreshBudgetSnapshot = useCallback(async () => {
    const current = await getSnapshot();
    setSnapshot(current);
    return current;
  }, []);
  const speedTests = useModelSpeedTests(refreshBudgetSnapshot, page === 'models' || (settingsOpen && settingsSection === 'gateway'));
  useEffect(() => { if (page !== 'models') speedTests.select([]); }, [page, speedTests.select]);
  useEffect(() => {
    let disposed=false; let unlisten: (() => void) | undefined;
    void listen<string>('gateway-lifecycle-error', event => { window.alert(event.payload); }).then(fn => { if(disposed)fn();else unlisten=fn; }).catch(() => {});
    return () => { disposed=true;unlisten?.(); };
  }, []);
  const [busy, setBusy] = useState(false);
  const [toast, setToastState] = useState('');
  const [toastError, setToastError] = useState(false);
  const [toastVersion, setToastVersion] = useState(0);
  const setToast = (message: string, error = false) => { setToastState(message); setToastError(error); setToastVersion(version => version + 1); };
  const [providerTests, setProviderTests] = useState<Record<string, ProviderTestStatus>>({});
  const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null);
  const [deletingProvider, setDeletingProvider] = useState(false);
  const [modelDeleteTarget, setModelDeleteTarget] = useState<Model | null>(null);
  const [deletingModel, setDeletingModel] = useState(false);
  const [providerModal, setProviderModal] = useState<Provider | null>(null);
  const [authTarget, setAuthTarget] = useState<Provider | null>(null);
  const [modelModal, setModelModal] = useState<Model | null>(null);

  useEffect(() => {
    getSnapshot().then(setSnapshot).catch((error) => setToast(String(error)));
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = setTimeout(() => setToast(''), 3200);
    return () => clearTimeout(timer);
  }, [toast, toastVersion]);

  const run = async (work: () => Promise<DashboardSnapshot>, message: string, propagate = false) => {
    setBusy(true);
    try {
      setSnapshot(await work());
      setToast(message);
    } catch (error) {
      if (propagate) throw error;
      setToast(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const changeGateway = async (mode: 'start' | 'pause' | 'stop') => {
    if (gatewayOperation.current) return;
    gatewayOperation.current = true; setGatewayBusy(true);
    try {
      setSnapshot(await (mode === 'pause' ? pauseProxy() : toggleProxy(mode === 'start')));
      setStopGatewayOpen(false);
      setToast(t(mode === 'pause' ? 'Gateway paused' : mode === 'stop' ? 'Gateway stopped; agent configurations restored' : 'Proxy started'));
    } catch (error) { setToast(error instanceof Error ? error.message : String(error), true); }
    finally { gatewayOperation.current = false; setGatewayBusy(false); }
  };

  if (!snapshot) {
    return (
      <div className="boot-screen">
        <div className="brand-mark"><BrandMark /></div>
        <span>{t("Starting local router…")}</span>
      </div>
    );
  }

  const navigation: Array<{ id: Page; label: string; icon: typeof Activity }> = [
    { id: 'overview', label: t("Overview"), icon: CircleGauge },
    { id: 'providers', label: t("Providers"), icon: Server },
    { id: 'models', label: t("Models"), icon: Cpu },
    { id: 'router', label: t("Routes"), icon: GitBranch },
    { id: 'agents', label: t("Agents"), icon: Bot },
    { id: 'debug', label: t("Debug"), icon: Play },
    { id: 'activity', label: t("Logs"), icon: Activity },
    { id: 'usage', label: t("Usage"), icon: ChartNoAxesCombined },
  ];

  return (
    <div className="app-shell">
      <WindowFrame/>
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark"><BrandMark /></div>
          <div><strong>AutoJev</strong><span>{t("MODEL ROUTER")}</span></div>
        </div>

        <nav>
          {navigation.map((item) => {
            const Icon = item.icon;
            return (
              <button key={item.id} data-nav-page={item.id} className={cx('nav-item', page === item.id && 'active')} onClick={() => setPage(item.id)}>
                <Icon size={17} />
                <span>{item.label}</span>
                
              </button>
            );
          })}
        </nav>

        {stopGatewayOpen && <GatewayActionMenu anchor={gatewayButton} busy={gatewayBusy} onClose={() => { if (!gatewayBusy) setStopGatewayOpen(false); }} onConfirm={pause => void changeGateway(pause ? 'pause' : 'stop')}/>}
        <div className="sidebar-bottom">
          <div className="proxy-mini">
            <div className="status-line"><span className={cx('status-dot', snapshot.proxy.running && !snapshot.proxy.paused && 'online')} />{snapshot.proxy.paused ? t("Gateway paused") : snapshot.proxy.running ? t("Proxy online") : t("Proxy stopped")}</div>
            </div>
          <button ref={gatewayButton} aria-haspopup={snapshot.proxy.running && !snapshot.proxy.paused ? "menu" : undefined} aria-expanded={stopGatewayOpen} aria-controls={stopGatewayOpen ? "gateway-action-menu" : undefined} className="icon-action" title={t(snapshot.proxy.running && !snapshot.proxy.paused ? 'Stop proxy' : 'Start proxy')} aria-label={t(snapshot.proxy.running && !snapshot.proxy.paused ? 'Stop proxy' : 'Start proxy')} aria-busy={gatewayBusy} disabled={busy || gatewayBusy} onClick={() => snapshot.proxy.running && !snapshot.proxy.paused ? setStopGatewayOpen(open => !open) : void changeGateway('start')}>
            {gatewayBusy ? <LoaderCircle size={17} className="import-spinner"/> : snapshot.proxy.running && !snapshot.proxy.paused ? <Power size={17}/> : <Play size={17}/>}
          </button>
        </div>
        <button className="settings-trigger" onClick={() => setSettingsOpen(true)}><Settings2 size={17} /><span>{t('Settings')}</span>{update.update ? <span className="settings-update-dot" /> : <kbd>⌘ / Ctrl ,</kbd>}</button>
      </aside>

      <main className={page === 'debug' ? 'debug-main' : undefined}>
        <div className="page-content">
          {page === 'overview' && <Overview snapshot={snapshot} go={setPage} />}
          {page === 'providers' && (
            <ProvidersPage
              snapshot={snapshot}
              onImport={async (source) => {
                setBusy(true);
                try {
                  const result = await importProviders(source);
                  setSnapshot(result.snapshot);
                  const message = t('{imported} providers imported; {skipped} existing or incompatible entries skipped.', { imported: result.imported, skipped: result.skipped });
                  setToast(message);
                } catch (error) {
                  setToast(String(error));
                  getSnapshot().then(setSnapshot).catch(() => {});
                } finally { setBusy(false); }
              }}
              onAdd={() => setProviderModal({ ...EMPTY_PROVIDER })}
              onEdit={(provider) => setProviderModal({ ...provider })}
              onDelete={(id) => setDeleteTarget(snapshot.providers.find((provider) => provider.id === id) ?? null)}
              onToggle={async (provider) => {
                try {
                  setSnapshot(await saveProvider({ ...provider, enabled: !provider.enabled }));
                  setToast(t(provider.enabled ? 'Provider disabled' : 'Provider enabled'));
                } catch (error) { setToast(String(error), true); }
              }}
              testStates={providerTests}
              onAuth={(provider) => setAuthTarget(provider)}
              onSnapshot={setSnapshot}
              onNotify={setToast}
              onRefreshSubscription={async (provider) => {
                try {
                  setSnapshot(await refreshSubscription(provider.id));
                  setToast(t('Read-only status refreshed'));
                } catch (error) {
                  setToast(String(error instanceof Error ? error.message : error), true);
                  // 刷新失败也可能已撤销当前连接并保留历史，立即展示落盘状态。
                  getSnapshot().then(setSnapshot).catch(() => {});
                }
              }}
              onTest={async (id) => {
                setProviderTests((previous) => ({ ...previous, [id]: 'testing' }));
                try {
                  const result = await testProvider(id);
                  setProviderTests((previous) => ({ ...previous, [id]: 'success' }));
                  setToast(t(result));
                } catch (error) {
                  setProviderTests((previous) => ({ ...previous, [id]: 'error' }));
                  setToast(providerTestError(error, t), true);
                } finally {
                  // Any admitted test attempt consumes the shared per-connection count, even when it fails.
                  try { await refreshBudgetSnapshot(); } catch { /* Keep the test result visible. */ }
                }
              }}
            />
          )}
          {page === 'models' && (
            <ModelsPage
              onNotify={setToast}
              onChange={setSnapshot}
              snapshot={snapshot}
              speedTests={speedTests}
              onRefreshBudgetSnapshot={refreshBudgetSnapshot}
              onAdd={() => setModelModal({ ...EMPTY_MODEL, provider_id: snapshot.providers.find((provider) => provider.enabled)?.id ?? '' })}
              onEdit={(model) => setModelModal({ ...model })}
              onDelete={(id) => setModelDeleteTarget(snapshot.models.find((model) => model.id === id) ?? null)}
            />
          )}
          {page === 'router' && <RoutesPage snapshot={snapshot} onChange={setSnapshot} />}
          {page === 'agents' && (
            <AgentsPage
              onChange={setSnapshot}
              onManageRoutes={() => setPage('router')}
              connecting={busy}
              snapshot={snapshot}
              onRefresh={async () => {
                setBusy(true);
                try { setSnapshot({ ...snapshot, agents: await detectAgents() }); setToast(t("Agent status refreshed")); } finally { setBusy(false); }
              }}
              onConnect={(id, routeId, routeIds) => run(() => connectAgent(id, routeId, routeIds), t("Agent connected to AutoJev"), true)}
              onRestore={(id) => run(() => restoreAgent(id), t("Original agent configuration restored"), true)}
            />
          )}
          {page === 'activity' && <TrafficPage key="logs" mode="logs" />}
          {page === 'debug' && <DebugPage snapshot={snapshot} onRefreshBudgetSnapshot={refreshBudgetSnapshot} />}
          {page === 'usage' && <TrafficPage key="usage" mode="usage" />}
        </div>
      </main>

      {settingsOpen && <SettingsDialog snapshot={snapshot} onChange={setSnapshot} speedTests={speedTests} section={settingsSection} onSectionChange={setSettingsSection} onClose={() => setSettingsOpen(false)} update={update} />}
      {deleteTarget && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !deletingProvider) setDeleteTarget(null); }} onKeyDown={(event) => { if (event.key === 'Escape' && !deletingProvider) setDeleteTarget(null); }}>
        <div className="dialog provider-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-provider-title" aria-describedby="delete-provider-description">
          <h2 id="delete-provider-title">{t('Delete provider?')}</h2>
          <p id="delete-provider-description">{t('Delete {provider}? Its saved API key and associated models will also be removed. This cannot be undone.', { provider: deleteTarget.name })}</p>
          <div className="provider-dialog-actions"><button autoFocus type="button" className="button ghost" disabled={deletingProvider} onClick={() => setDeleteTarget(null)}>{t('Cancel')}</button><button type="button" className="button destructive" disabled={deletingProvider} onClick={async () => {
            if (deletingProvider) return;
            setDeletingProvider(true);
            try { setSnapshot(await deleteProvider(deleteTarget.id)); setDeleteTarget(null); setToast(t('Provider removed')); }
            catch (error) { setToast(String(error), true); }
            finally { setDeletingProvider(false); }
          }}>{deletingProvider ? <LoaderCircle size={15} className="import-spinner" /> : <Trash2 size={15} />}{t(deletingProvider ? 'Deleting…' : 'Delete')}</button></div>
        </div>
      </div>}
      {modelDeleteTarget && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !deletingModel) setModelDeleteTarget(null); }} onKeyDown={(event) => { if (event.key === 'Escape' && !deletingModel) setModelDeleteTarget(null); }}>
        <div className="dialog provider-delete-dialog" role="alertdialog" aria-modal="true" aria-labelledby="delete-model-title" aria-describedby="delete-model-description">
          <h2 id="delete-model-title">{t('Delete model?')}</h2>
          <p id="delete-model-description">{t('Delete {model}? This cannot be undone.', { model: modelDeleteTarget.name || modelDeleteTarget.model_id })}</p>
          <div className="provider-dialog-actions"><button autoFocus type="button" className="button ghost" disabled={deletingModel} onClick={() => setModelDeleteTarget(null)}>{t('Cancel')}</button><button type="button" className="button destructive" disabled={deletingModel} onClick={async () => {
            if (deletingModel) return;
            setDeletingModel(true);
            try { setSnapshot(await deleteModel(modelDeleteTarget.id)); setModelDeleteTarget(null); setToast(t('Model removed')); }
            catch (error) { setToast(String(error), true); }
            finally { setDeletingModel(false); }
          }}>{deletingModel ? <LoaderCircle size={15} className="import-spinner" /> : <Trash2 size={15} />}{t(deletingModel ? 'Deleting…' : 'Delete')}</button></div>
        </div>
      </div>}
      {providerModal && (
        <ProviderDialog
          onTestStatus={async (status) => { if (providerModal.id) setProviderTests((previous) => ({ ...previous, [providerModal.id]: status })); if (status === 'success' || status === 'error') { try { await refreshBudgetSnapshot(); } catch { /* Keep the test result visible. */ } } }}
          initial={providerModal}
          initialSource={snapshot.api_sources?.[providerModal.id]}
          subscriptionTargets={providerModal.id ? subscriptionCatalog(subscriptionView(snapshot, providerModal.id)) : []}
          onClose={() => setProviderModal(null)}
          onSave={async (provider, apiKey, addTestModel, source) => {
            setSnapshot(await saveProvider(provider, apiKey, addTestModel, providerModal.id || undefined, !providerModal.id, source));
            setProviderModal(null);
            setToast(t("Provider saved"));
          }}
        />
      )}
      {authTarget && snapshot && <SubscriptionAuthDialog
        provider={authTarget}
        snapshot={snapshot}
        onSnapshot={setSnapshot}
        onNotify={setToast}
        onClose={() => setAuthTarget(null)}
      />}
      {modelModal && (
        <ModelDialog
          onTestStatus={async (id, status) => { setProviderTests((previous) => ({ ...previous, [id]: status })); if (status === 'success' || status === 'error') { try { await refreshBudgetSnapshot(); } catch { /* Keep the test result visible. */ } } }}
          initial={modelModal}
          providers={snapshot.providers}
          onClose={() => setModelModal(null)}
          onSave={async (model) => {
            setSnapshot(await saveModel(model));
            setModelModal(null);
            setToast(t('Model saved'));
          }}
        />
      )}
      {toast && <div className={cx("toast", toastError && "toast-error")} role={toastError ? "alert" : "status"} aria-live="polite">{toastError ? <X size={16} /> : <Check size={16} />}{toast}</div>}
    </div>
  );
}

function Overview({ snapshot, go }: { snapshot: DashboardSnapshot; go: (page: Page) => void }) {
  const { t } = usePreferences();
  const providers = snapshot.providers.filter((provider) => provider.enabled);
  // 未选模型不进入列表与计数；停用模型仍算列表成员，只是不参与路由。
  const pool = modelListMembers(snapshot.models);
  const models = pool.filter((model) => model.enabled && providers.some((provider) => provider.id === model.provider_id));
  const [today, setToday] = useState<{ requests: number; tokens: number } | null>(null);
  useEffect(() => {
    let disposed = false;
    let pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      try {
        const totals = summarize(await getRequestLogs(rangeStart(1).toISOString()));
        if (!disposed) setToday({ requests: totals.requests, tokens: totals.input + totals.output });
      } catch {
        if (!disposed) setToday(null);
      } finally { pending = false; }
    };
    void refresh();
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, 5000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, []);
  const agents = snapshot.agents.filter((agent) => agent.connected);
  const running = snapshot.proxy.running && !snapshot.proxy.paused;

  return <div className="router-home">
    <section className="overview-intro">
      <p>{t('Route every request to the right model.')}</p>
      <button className="button ghost" onClick={() => go('agents')}><Bot size={16} />{t('Connect an agent')}</button>
    </section>

    <section className="router-map" aria-label={t('Routing topology')}>
      <div className="router-map-bar"><span><Network size={14} /><code>127.0.0.1:{snapshot.proxy.port}</code></span><span className="status-line"><i className={cx('status-dot', running && 'online')} />{t(snapshot.proxy.paused ? 'Gateway paused' : running ? 'Proxy online' : 'Proxy stopped')}</span></div>
      <div className={cx("router-map-body", running && "routing-active")}>
        <div className="router-endpoints">
          <span className="router-map-label">{t('Agents')}</span>
          {(agents.length ? agents.slice(0, 3) : [{ id: 'empty', name: t('Connect an agent') }]).map((agent) => <button className="router-endpoint" key={agent.id} onClick={() => go('agents')}><span className="router-endpoint-logo">{agent.id === 'empty' ? <Bot size={21} /> : <AgentLogo id={agent.id} />}</span><strong>{agent.name}</strong><ChevronRight size={14} /></button>)}
          <small>{agents.length ? t('{count} connected', { count: agents.length }) : 'Codex · Claude Code · …'}</small>
        </div>
        <div className="router-wire" aria-hidden="true"><ArrowRight size={14} /></div>
        <button className="router-hub" onClick={() => go('router')} aria-label={t('Manage routes')}>
          <span className="router-hub-icon"><BrandMark /></span>
          <strong>AutoJev</strong>
        </button>
        <div className="router-wire" aria-hidden="true"><ArrowRight size={14} /></div>
        <div className="router-endpoints">
          <span className="router-map-label">{t('Models')}</span>
          {models.slice(0, 3).map((model) => <button className="router-endpoint" key={model.id} onClick={() => go('models')} title={model.name || model.model_id}>
            <span className="router-endpoint-logo"><ProviderLogo id={providerPreset(providers.find((provider) => provider.id === model.provider_id)!)} /></span><span><strong>{model.name || model.model_id}</strong><small>{providers.find((provider) => provider.id === model.provider_id)?.name}</small></span><ChevronRight size={14} />
          </button>)}
          {!models.length && <button className="router-endpoint" onClick={() => go('models')}><Plus size={21} /><strong>{t('Add model')}</strong><ChevronRight size={14} /></button>}
          <small>{t('{count} models connected', { count: models.length })}</small>
        </div>
      </div>
    </section>

    <section className="router-home-summary" aria-label={t('Overview')}>
      {([
        { page: 'providers', label: 'Providers', count: snapshot.providers.length, icon: Server },
        { page: 'models', label: 'Models', count: pool.length, icon: Cpu },
        { page: 'router', label: 'Routes', count: snapshot.routes.length, icon: GitBranch },
        { page: 'agents', label: 'Agents', count: snapshot.agents.filter((agent) => agent.installed || agent.custom).length, icon: Bot },
        { page: 'activity', label: 'Requests today', count: today?.requests, icon: Activity },
        { page: 'usage', label: 'Tokens today', count: today?.tokens, icon: ChartNoAxesCombined },
      ] as const).map(({ page, label, count, icon: Icon }) => <button key={page} onClick={() => go(page)}>
        <Icon size={20} /><span>{t(label)}</span><strong title={count?.toLocaleString()}>{count === undefined ? '—' : page === 'usage' ? new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(count) : count.toLocaleString()}</strong><ChevronRight size={15} />
      </button>)}
    </section>
  </div>;
}

function ProviderImport({ onImport }: { onImport: (source: 'ccswitch' | 'termany') => Promise<void> }) {
  const { t } = usePreferences();
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const inFlight = useRef(false);
  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!container.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, []);
  const start = async (source: 'ccswitch' | 'termany') => {
    if (inFlight.current) return;
    inFlight.current = true;
    setOpen(false);
    trigger.current?.focus();
    setPending(true);
    try { await onImport(source); }
    finally { inFlight.current = false; setPending(false); }
  };
  return <div ref={container} className="provider-import" onKeyDown={(event) => {
    if (event.key === 'Escape') { setOpen(false); trigger.current?.focus(); }
  }}>
    <button ref={trigger} type="button" className="button ghost" disabled={pending} aria-expanded={open} aria-controls="provider-import-options" onClick={() => setOpen(!open)}>
      {pending ? <LoaderCircle size={16} className="import-spinner" /> : <Download size={16} />}
      {pending ? t('Importing…') : t('Import')}<ChevronDown size={14} />
    </button>
    {open && <div id="provider-import-options" className="provider-import-menu">
      {(['ccswitch', 'termany'] as const).map((source) => <button key={source} type="button" onClick={() => void start(source)}>
        <Download size={14} />{t('Import from {source}', { source: source === 'ccswitch' ? 'CC Switch' : 'Termany' })}
      </button>)}
    </div>}

  </div>;
}

type SubscriptionAction = 'login' | 'cancel' | 'logout' | 'switch';

function ProvidersPage({ snapshot, onAdd, onEdit, onDelete, onTest, onImport, onRefreshSubscription, onAuth, onSnapshot, onNotify, testStates, onToggle }: { onToggle: (provider: Provider) => Promise<void>; onRefreshSubscription: (provider: Provider) => Promise<void>; onAuth: (provider: Provider) => void; onSnapshot: (snapshot: DashboardSnapshot) => void; onNotify: (message: string, error?: boolean) => void; testStates: Record<string, ProviderTestStatus>; onImport: (source: 'ccswitch' | 'termany') => Promise<void>; snapshot: DashboardSnapshot; onAdd: () => void; onEdit: (p: Provider) => void; onDelete: (id: string) => void; onTest: (id: string) => void }) {
  const { t } = usePreferences();
  const [toggling, setToggling] = useState<Record<string, boolean>>({});
  const [refreshing, setRefreshing] = useState<Record<string, boolean>>({});
  const [subscriptionBusy, setSubscriptionBusy] = useState<Record<string, SubscriptionAction | undefined>>({});
  const [subscriptionErrors, setSubscriptionErrors] = useState<Record<string, string>>({});
  const [generationBusy, setGenerationBusy] = useState<Record<string, boolean>>({});
  const [generationCallLimits, setGenerationCallLimits] = useState<Record<string, number>>({});
  const [catalogBusy, setCatalogBusy] = useState<Record<string, boolean>>({});
  // 目录行的选择/停用直接写回模型配置：复用 save_model，写回后以返回的快照为准刷新。
  const toggleCatalogEntry = async (provider: Provider, entry: SubscriptionCatalogEntry, patch: { selected?: boolean; enabled?: boolean }) => {
    const key = `${provider.id}:${entry.internal_id}`;
    if (catalogBusy[key]) return;
    const model = catalogModelRow(snapshot.models, entry);
    if (!model) { onNotify(t('This catalog entry has no local model row yet; refresh the snapshot.'), true); return; }
    setCatalogBusy((previous) => ({ ...previous, [key]: true }));
    try {
      onSnapshot(await saveModel({ ...model, ...patch }));
      onNotify(t(patch.selected !== undefined
        ? (patch.selected ? 'Model added to the model list' : 'Model removed from the model list')
        : (patch.enabled ? 'Model enabled' : 'Model disabled')));
    } catch (error) {
      onNotify(error instanceof Error ? error.message : String(error), true);
    } finally {
      setCatalogBusy((previous) => { const next = { ...previous }; delete next[key]; return next; });
    }
  };
  const toggle = async (provider: Provider) => {
    if (toggling[provider.id]) return;
    setToggling((previous) => ({ ...previous, [provider.id]: true }));
    try { await onToggle(provider); }
    finally { setToggling((previous) => ({ ...previous, [provider.id]: false })); }
  };
  const toggleCodexGeneration = async (provider: Provider, enabled: boolean) => {
    if (generationBusy[provider.id]) return;
    const currentView = subscriptionView(snapshot, provider.id);
    if (enabled && (!currentView || !currentView.identity?.trim())) {
      onNotify(t('A current connected account is required before enabling real Codex generation.'), true);
      return;
    }
    // Capture before showing the modal so the backend can reject consent after an account change.
    const expectedConnectionInstanceId = currentView?.connection_instance_id ?? '';
    const expectedGeneration = currentView?.generation ?? 0;
    const expectedIdentity = currentView?.identity ?? '';
    const generationKey = `${provider.id}:${currentView?.generation ?? 0}`;
    const maxCalls = generationCallLimits[generationKey] ?? currentView?.generation_call_limit ?? 1;
    if (enabled && (!Number.isInteger(maxCalls) || maxCalls < 1 || maxCalls > 15)) {
      onNotify(t('The request limit must be between 1 and 15.'), true);
      return;
    }
    if (enabled && !window.confirm(t('Before enabling real Codex generation, complete the HAND_RUN checklist and confirm the exact model and protocol, fictional input, client-owned tools, total request count, output boundary, possible fees, and allowed steps. This confirmation is limited to {count} AutoJev requests. Failed, cancelled, and client-tool follow-up requests count. The counter is not an upstream billing guarantee. This switch sends no request and does not bypass any admission check; the current 64 KiB collector is not a hard output cap. Continue?', { count: maxCalls }))) return;
    setGenerationBusy((previous) => ({ ...previous, [provider.id]: true }));
    try {
      onSnapshot(await setCodexRealGenerationEnabled(provider.id, enabled, maxCalls, expectedConnectionInstanceId, expectedGeneration, expectedIdentity));
      onNotify(t(enabled ? 'Real Codex generation armed for this connection.' : 'Real Codex generation disarmed.'));
    } catch (error) {
      onNotify(error instanceof Error ? error.message : String(error), true);
    } finally {
      setGenerationBusy((previous) => { const next = { ...previous }; delete next[provider.id]; return next; });
    }
  };
  const toggleGrokGeneration = async (provider: Provider, enabled: boolean) => {
    if (generationBusy[provider.id]) return;
    const currentView = subscriptionView(snapshot, provider.id);
    const loginSupported = subscriptionAuthView(snapshot, provider.id)?.helper?.login_supported === true;
    if (enabled && (!loginSupported || !currentView || currentView.state !== 'connected' || !currentView.identity?.trim())) {
      onNotify(t('A verified Grok login and current connected identity are required before arming generation.'), true);
      return;
    }
    const expectedConnectionInstanceId = currentView?.connection_instance_id ?? '';
    const expectedGeneration = currentView?.generation ?? 0;
    const expectedIdentity = currentView?.identity ?? '';
    const generationKey = `${provider.id}:${currentView?.generation ?? 0}`;
    const maxCalls = generationCallLimits[generationKey] ?? currentView?.generation_call_limit ?? 1;
    if (enabled && (!Number.isInteger(maxCalls) || maxCalls < 1 || maxCalls > 15)) {
      onNotify(t('The request limit must be between 1 and 15.'), true);
      return;
    }
    if (enabled && !window.confirm(t('Before arming real Grok generation, complete HAND_RUN. This is an account-wide opt-in for every eligible model and verified protocol on this connection; it does not lock access to the model and protocol in your plan. Confirm the specific model, protocol, fictional inputs, client-owned tools, dispatch and helper-turn limit, output boundary, possible fees, and allowed steps in HAND_RUN, and use only that plan. The limit is {count} AutoJev requests; failures, cancellations, helper turns, and client-tool follow-ups count. This volatile switch sends no request and cannot bypass identity, model, protocol, quota, or Extra Usage admission. The production Grok adapter remains closed until its interface is verified. Continue?', { count: maxCalls }))) return;
    setGenerationBusy((previous) => ({ ...previous, [provider.id]: true }));
    try {
      onSnapshot(await setGrokRealGenerationEnabled(provider.id, enabled, maxCalls, expectedConnectionInstanceId, expectedGeneration, expectedIdentity));
      onNotify(t(enabled ? 'Real Grok generation armed for this connection.' : 'Real Grok generation disarmed.'));
    } catch (error) {
      onNotify(error instanceof Error ? error.message : String(error), true);
    } finally {
      setGenerationBusy((previous) => { const next = { ...previous }; delete next[provider.id]; return next; });
    }
  };
  const refresh = async (provider: Provider) => {
    if (refreshing[provider.id]) return;
    setRefreshing((previous) => ({ ...previous, [provider.id]: true }));
    try { await onRefreshSubscription(provider); }
    finally { setRefreshing((previous) => ({ ...previous, [provider.id]: false })); }
  };
  // 挂起登录只由既有 get_snapshot 轮询观察，不新建事件通道。
  const loginPending = (snapshot.subscriptions ?? []).some((view) => view.login?.stage === 'pending');
  useEffect(() => {
    if (!loginPending) return;
    let stopped = false;
    let inFlight = false;
    const timer = setInterval(() => {
      if (inFlight) return;
      inFlight = true;
      getSnapshot()
        .then((next) => { if (!stopped) onSnapshot(next); })
        .catch(() => { /* 保留上一次已知状态，等待下一次轮询。 */ })
        .finally(() => { inFlight = false; });
    }, 2000);
    return () => { stopped = true; clearInterval(timer); };
  }, [loginPending, onSnapshot]);
  const runSubscriptionAction = async (provider: Provider, action: SubscriptionAction) => {
    if (subscriptionBusy[provider.id]) return;
    setSubscriptionBusy((previous) => ({ ...previous, [provider.id]: action }));
    setSubscriptionErrors((previous) => { const next = { ...previous }; delete next[provider.id]; return next; });
    try {
      if (action === 'login') { onSnapshot(await beginSubscriptionLogin(provider.id)); onNotify(t('Subscription sign-in started')); }
      else if (action === 'cancel') { onSnapshot(await cancelSubscriptionLogin(provider.id)); onNotify(t('Subscription sign-in cancelled')); }
      else if (action === 'logout') { onSnapshot(await logoutSubscription(provider.id)); onNotify(t('Subscription signed out')); }
      else { onSnapshot(await switchSubscriptionAccount(provider.id)); onNotify(t('Subscription account switched')); }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setSubscriptionErrors((previous) => ({ ...previous, [provider.id]: message }));
      onNotify(message, true);
    } finally {
      setSubscriptionBusy((previous) => { const next = { ...previous }; delete next[provider.id]; return next; });
    }
  };
  const providers = sortProviders(snapshot.providers.filter(p=>!snapshot.cpa_subscriptions?.some(c=>c.provider_id===p.id)), testStates);
  return (
    <div className="stack lg">
      <PageIntro title={t("Providers")} body={t("Keys are stored in the local database.")} action={<div className="provider-actions"><ProviderImport onImport={onImport} /><button className="button primary" onClick={onAdd}><Plus size={16} /> {t("Add provider")}</button></div>} />
      <GrokReadOnlyStatus t={t} />
      <CpaDevelopmentService />
      <CpaSubscriptions connections={snapshot.cpa_subscriptions??[]} onSnapshot={onSnapshot} onNotify={onNotify} />
      <CodingPlanHandRuns connections={snapshot.coding_hand_runs??[]} onSnapshot={onSnapshot} onNotify={onNotify} />
      <div className="table-panel provider-table-panel">
        <table className="provider-table">
          <thead><tr><th>{t('Provider')}</th><th>{t('Base URL')}</th><th>{t('Enabled status')}</th><th>{t('Subscription')}</th><th>{t('API key')}</th><th className="provider-actions-heading">{t('Actions')}</th></tr></thead>
          <tbody>
            {providers.map((provider) => {
              const subscription = isSubscriptionProvider(provider);
              // 两套订阅 UI 按 kind 分派：Grok 由本票的授权对话框承载，Codex 沿用 #13 的行内动作。
              const grok = provider.kind === 'grok_subscription';
              const view = subscription ? subscriptionView(snapshot, provider.id) : undefined;
              const auth = subscription ? subscriptionAuthView(snapshot, provider.id) : undefined;
              const available = subscriptionActions(view?.state ?? 'not_connected');
              const busyAction = subscriptionBusy[provider.id];
              const login = view?.login ?? null;
              const awaitingAuthorization = login?.stage === 'pending';
              const authorizationUrl = awaitingAuthorization ? login?.authorization_url ?? null : null;
              const catalog = view?.catalog;
              const quota = view?.quota;
              const quotaHistory = quotaHistoryLabel(quota, t);
              const catalogRemoved = catalogRemovedLabel(catalog?.removed_models, t);
              // 官方查看入口只属于确有可靠入口的服务商类型（当前只有 Grok）。判断依据是服务商类型，
              // 不是名称或标识；Codex 没有已核实的官方入口就不新增，不猜 URL。
              const catalogState = catalog?.state ?? 'unknown';
              const quotaState = quota?.state ?? 'unknown';
              const providerTestDisabled = testStates[provider.id] === 'testing' || (subscription && !provider.test_model?.trim());
              const providerTestTitle = testStates[provider.id] === 'testing' ? 'Testing…'
                : subscription && !provider.test_model?.trim() ? 'Choose a discovered test model in Configure first.'
                  : 'Test';
              const showCatalogReference = grok && (catalogState === 'unsupported' || catalogState === 'unknown');
              const showQuotaReference = grok && (quotaState === 'unsupported' || quotaState === 'unknown');
              const openReference = (url: string) => (event: { preventDefault: () => void }) => {
                // Tauri 下拦截默认跳转，交给系统浏览器；普通浏览器保留正常链接行为。
                if (!isTauri()) return;
                event.preventDefault();
                void openUrl(url).catch(() => onNotify(t('Open official reference'), true));
              };
              return (
              <tr key={provider.id}>
                <td><div className="provider-table-name"><span className="provider-table-icon"><ProviderLogo id={providerPreset(provider)} /></span><div><strong>{provider.name}</strong><small>{provider.id}</small>{snapshot.api_sources?.[provider.id] && <small data-testid={`source-identity-${provider.id}`}>{t(API_SOURCE_LABELS[snapshot.api_sources[provider.id].kind])} · {snapshot.api_sources[provider.id].plan_label || t('Unknown')} · {t('Plan eligibility and fees unknown')}</small>}</div></div></td>
                <td>{subscription ? <span className="provider-subscription-identity" title={t('Subscription identity')}><ShieldCheck size={14} aria-hidden="true" />{identityLabel(view, t)}</span> : <code className="provider-table-url" title={provider.base_url}>{provider.base_url}</code>}</td>
                <td><div className="provider-enabled-cell"><button type="button" role="switch" aria-checked={provider.enabled} aria-label={t('Enable {provider}', { provider: provider.name })} disabled={toggling[provider.id]} className={cx('switch', provider.enabled && 'on')} onClick={() => void toggle(provider)}><span /></button><span>{t(provider.enabled ? 'ENABLED' : 'DISABLED')}</span></div></td>
                <td>{subscription ? <div className="provider-subscription-status"><span className={cx('provider-connection', connectionStateTone(view?.state ?? 'not_connected'))}><Plug size={14} aria-hidden="true" />{connectionStateLabel(view?.state ?? 'not_connected', t)}</span><span className="provider-subscription-detail">{view && view.models.length > 0 ? t('{count} discovered models', { count: view.models.length }) : t('No discovered models yet')}</span><span className="provider-subscription-state" data-testid={`sub-status-${provider.id}`}>{subscriptionStatusText(view, t)}</span>{login && login.stage !== 'idle' && <span className={cx('provider-subscription-login', loginStageTone(login.stage))} role="status">{loginStageLabel(login.stage, t)}</span>}{authorizationUrl && <a className="provider-subscription-auth-link" href={authorizationUrl} target="_blank" rel="noreferrer" onClick={(event) => { if (isTauri()) { event.preventDefault(); void openUrl(authorizationUrl).catch(() => onNotify(t('Open authorization link'), true)); } }}>{t('Open authorization link')}</a>}{awaitingAuthorization && login?.user_code && <span className="provider-subscription-user-code"><span>{t('Authorization code')}</span><code>{login.user_code}</code></span>}{awaitingAuthorization && !authorizationUrl && !login?.user_code && <span className="provider-subscription-login">{t('Complete the authorization in your browser. This row updates automatically.')}</span>}{login?.error && <span className="provider-subscription-error" role="status">{login.error}</span>}{view?.logout && <span className="provider-subscription-logout" role="status">{localLogoutLabel(view.logout.local, t)} · {remoteRevocationLabel(view.logout.remote, t)}</span>}{subscriptionErrors[provider.id] && <span className="provider-subscription-error" role="status">{subscriptionErrors[provider.id]}</span>}{subscriptionReason(view, t) && <span className="provider-subscription-reason" role="status" title={subscriptionReason(view, t)}>{denialLabel(view?.denial ?? view?.admission_denial, t) || quotaLabel(view?.quota?.state ?? 'unknown', t)}</span>}{subscription && <span className="provider-subscription-catalog" data-testid={`sub-catalog-${provider.id}`}>{subscriptionCatalogText(view)}</span>}{subscription && <span className="provider-subscription-catalog-human">{t('Model catalog')} · {catalogLabel(catalog?.state ?? 'unknown', t)} · {catalog?.observed_at?.trim() || t('Unknown')}{catalog?.source?.trim() ? ` · ${catalog.source.trim()}` : ''}</span>}{subscription && <span className="provider-subscription-quota" data-testid={`sub-quota-${provider.id}`}>{subscriptionQuotaText(view)}</span>}{subscription && <span className="provider-subscription-quota-human">{quotaLabel(quota?.state ?? 'unknown', t)} · {quotaViewLabel(quota?.view, t)}</span>}{subscription && (quota?.buckets ?? []).map((bucket) => <span key={`${provider.id}-${bucket.limit_id}`} className="provider-subscription-quota-bucket">{quotaPermissionLabel(bucket.permission, t)}{bucket.credits?.balance?.trim() ? <code title={t('Raw balance text as reported; the unit is not inferred.')}>{bucket.credits.balance.trim()}</code> : null}<span className="provider-subscription-credits">{creditsAxisText(bucket.credits, t)}</span></span>)}{subscription && !(quota?.buckets ?? []).length && <span className="provider-subscription-quota-bucket">{quotaPermissionLabel(undefined, t)}</span>}{subscription && catalogRemoved && <span className="provider-subscription-removed" role="status">{catalogRemoved}</span>}{subscription && quotaHistory && <span className="provider-subscription-quota-history" role="status">{quotaHistory}</span>}{showCatalogReference && <a className="provider-subscription-reference" href={CATALOG_REFERENCE_URL} target="_blank" rel="noreferrer" title={CATALOG_REFERENCE_URL} onClick={openReference(CATALOG_REFERENCE_URL)}>{t('Official catalog reference')}</a>}{showQuotaReference && <a className="provider-subscription-reference" href={QUOTA_REFERENCE_URL} target="_blank" rel="noreferrer" title={QUOTA_REFERENCE_URL} onClick={openReference(QUOTA_REFERENCE_URL)}>{t('Official quota reference')}</a>}{(showCatalogReference || showQuotaReference) && <span className="provider-subscription-evidence-note">{t('Viewing the official reference is not a bypass for admission.')}</span>}</div> : <span className="provider-table-key">{t('Not a subscription provider')}</span>}</td>
                <td><span className="provider-table-key"><KeyRound size={14} />{subscription ? t('No API key is used for subscription providers.') : provider.kind === 'ollama' ? t('No API key required') : provider.has_api_key ? t('API key saved locally') : t('API key required')}</span></td>
                <td><div className="row-actions">{subscription && grok && <button className="icon-action subscription-auth-entry" aria-busy={auth?.phase === 'pending'} title={t('Subscription sign-in and sign-out')} aria-label={t('Subscription sign-in and sign-out')} onClick={() => onAuth(provider)}>{auth?.phase === 'pending' ? <LoaderCircle size={15} className="import-spinner" /> : view?.state === 'connected' ? <LogOut size={15} /> : <LogIn size={15} />}</button>}{subscription && !grok && <><button type="button" className="button ghost small subscription-action" data-testid={`sub-login-${provider.id}`} disabled={!available.canLogin || busyAction !== undefined} title={t('Sign in to subscription')} onClick={() => void runSubscriptionAction(provider, 'login')}>{busyAction === 'login' ? <LoaderCircle size={14} className="import-spinner" /> : null}{t('Sign in')}</button><button type="button" className="button ghost small subscription-action" data-testid={`sub-cancel-${provider.id}`} disabled={!available.canCancel || busyAction !== undefined} title={t('Cancel subscription sign-in')} onClick={() => void runSubscriptionAction(provider, 'cancel')}>{busyAction === 'cancel' ? <LoaderCircle size={14} className="import-spinner" /> : null}{t('Cancel')}</button><button type="button" className="button ghost small subscription-action" data-testid={`sub-logout-${provider.id}`} disabled={!available.canLogout || busyAction !== undefined} title={t('Sign out of subscription')} onClick={() => void runSubscriptionAction(provider, 'logout')}>{busyAction === 'logout' ? <LoaderCircle size={14} className="import-spinner" /> : null}{t('Sign out')}</button><button type="button" className="button ghost small subscription-action" data-testid={`sub-switch-${provider.id}`} disabled={!available.canLogout || busyAction !== undefined} title={t('Switch subscription account')} onClick={() => void runSubscriptionAction(provider, 'switch')}>{busyAction === 'switch' ? <LoaderCircle size={14} className="import-spinner" /> : null}{t('Switch account')}</button></>}{subscription && <button className="icon-action" data-testid={`sub-refresh-${provider.id}`} disabled={refreshing[provider.id]} title={t('Refresh read-only status')} aria-label={t('Refresh read-only status')} onClick={() => void refresh(provider)}>{refreshing[provider.id] ? <LoaderCircle size={15} className="import-spinner" /> : <RefreshCw size={15} />}</button>}<button type="button" className="icon-action" data-testid={`provider-test-${provider.id}`} disabled={providerTestDisabled} title={t(providerTestTitle)} aria-label={t(providerTestTitle)} onClick={() => onTest(provider.id)}>{testStates[provider.id] === 'testing' ? <LoaderCircle size={15} className="import-spinner" /> : <Play size={15} />}</button><button className="icon-action" data-testid={`provider-configure-${provider.id}`} onClick={() => onEdit(provider)} title={t('Configure')} aria-label={t('Configure')}><Settings2 size={15} /></button><button className="icon-action danger" onClick={() => onDelete(provider.id)} title={t('Delete')} aria-label={t('Delete')}><Trash2 size={15} /></button></div></td>
              </tr>
            ); })}
          </tbody>
        </table>
        {snapshot.providers.filter((provider) => provider.kind === 'codex_subscription' && !snapshot.cpa_subscriptions?.some(c=>c.provider_id===provider.id)).map((provider) => {
          const view = subscriptionView(snapshot, provider.id);
          const enabled = view?.real_generation_enabled === true;
          const canArm = provider.enabled && view?.state === 'connected' && Boolean(view.identity?.trim());
          const generationKey = `${provider.id}:${view?.generation ?? 0}`;
          const maxCalls = generationCallLimits[generationKey] ?? view?.generation_call_limit ?? 1;
          return <section className="provider-subscription-generation" key={`codex-generation-${provider.id}`} data-testid={`codex-generation-control-${provider.id}`} aria-label={t('Real Codex generation')}>
            <div>
              <strong>{t('Real Codex generation')}</strong>
              <p data-testid={`codex-generation-state-${provider.id}`}>{t(enabled ? 'Armed for this account; admission still required.' : 'Off. Real Codex requests are denied by default.')}</p>
              <small>{t('Connection generation {generation} · helper {helper}', { generation: view?.generation ?? 0, helper: view?.helper_version || t('Unknown') })} · {t('Helper self-report (unverified)')}: {view?.helper?.user_agent || t('Unknown')}</small>
            </div>
            <label className="provider-subscription-generation-limit">{t('Maximum requests for this confirmation')}
              <input
                type="number"
                min={1}
                max={15}
                step={1}
                inputMode="numeric"
                data-testid={`codex-generation-call-limit-${provider.id}`}
                aria-label={t('Maximum requests for this confirmation')}
                disabled={!canArm || view?.generation_call_limit != null || generationBusy[provider.id] === true}
                value={maxCalls}
                onChange={(event) => { const value = Number(event.currentTarget.value); setGenerationCallLimits((previous) => ({ ...previous, [generationKey]: value })); }}
              />
            </label>
            <label className="provider-subscription-generation-toggle">
              <input
                type="checkbox"
                data-testid={`codex-real-generation-${provider.id}`}
                checked={enabled}
                disabled={!canArm || generationBusy[provider.id] === true || (!enabled && view?.generation_calls_remaining === 0)}
                onChange={(event) => void toggleCodexGeneration(provider, event.currentTarget.checked)}
              />
              <span>{t('Enable real Codex generation for this connection')}</span>
            </label>
            <small className="provider-subscription-generation-budget" data-testid={`codex-generation-budget-${provider.id}`}>
              {view?.generation_call_limit != null
                ? t('{remaining} of {limit} confirmed AutoJev requests remain.', { remaining: view?.generation_calls_remaining ?? 0, limit: view?.generation_call_limit ?? maxCalls })
                : t('Each confirmation is limited to 1–15 AutoJev requests; failures, cancellations, and tool-result follow-ups count.')}
            </small>
            <p className="provider-subscription-generation-note">{t('This volatile control does not verify identity, models, protocols, quota, or credits. Every existing admission check remains mandatory. The per-connection request count survives disarm/re-arm and resets after restart, sign-out, or account switch.')}</p>
          </section>;
        })}
        {snapshot.providers.filter((provider) => provider.kind === 'grok_subscription' && !snapshot.cpa_subscriptions?.some(c=>c.provider_id===provider.id)).map((provider) => {
          const view = subscriptionView(snapshot, provider.id);
          const enabled = view?.real_generation_enabled === true;
          const loginSupported = subscriptionAuthView(snapshot, provider.id)?.helper?.login_supported === true;
          const canArm = provider.enabled && loginSupported && view?.state === 'connected' && Boolean(view.identity?.trim());
          const generationKey = `${provider.id}:${view?.generation ?? 0}`;
          const maxCalls = generationCallLimits[generationKey] ?? view?.generation_call_limit ?? 1;
          return <section className="provider-subscription-generation" key={`grok-generation-${provider.id}`} data-testid={`grok-generation-control-${provider.id}`} aria-label={t('Real Grok generation')}>
            <div>
              <strong>{t('Real Grok generation')}</strong>
              <p data-testid={`grok-generation-state-${provider.id}`}>{t(enabled ? 'Armed for this account; admission still required.' : 'Off. Real Grok requests are denied by default.')}</p>
              <small>{t('Connection generation {generation} · helper {helper}', { generation: view?.generation ?? 0, helper: view?.helper_version || t('Unknown') })} · {t('Helper self-report (unverified)')}: {view?.helper?.user_agent || t('Unknown')}</small>
            </div>
            <label className="provider-subscription-generation-limit">{t('Maximum requests for this confirmation')}
              <input
                type="number"
                min={1}
                max={15}
                step={1}
                inputMode="numeric"
                data-testid={`grok-generation-call-limit-${provider.id}`}
                aria-label={t('Maximum requests for this confirmation')}
                disabled={!canArm || view?.generation_call_limit != null || generationBusy[provider.id] === true}
                value={maxCalls}
                onChange={(event) => { const value = Number(event.currentTarget.value); setGenerationCallLimits((previous) => ({ ...previous, [generationKey]: value })); }}
              />
            </label>
            <label className="provider-subscription-generation-toggle">
              <input
                type="checkbox"
                data-testid={`grok-real-generation-${provider.id}`}
                checked={enabled}
                disabled={!canArm || generationBusy[provider.id] === true || (!enabled && view?.generation_calls_remaining === 0)}
                onChange={(event) => void toggleGrokGeneration(provider, event.currentTarget.checked)}
              />
              <span>{t('Enable real Grok generation for this connection')}</span>
            </label>
            <small className="provider-subscription-generation-budget" data-testid={`grok-generation-budget-${provider.id}`}>
              {view?.generation_call_limit != null
                ? t('{remaining} of {limit} confirmed AutoJev requests remain.', { remaining: view?.generation_calls_remaining ?? 0, limit: view?.generation_call_limit ?? maxCalls })
                : t('Each confirmation is limited to 1–15 AutoJev requests; failures, cancellations, helper turns, and client-tool follow-ups count.')}
            </small>
            <p className="provider-subscription-generation-note">{t('This volatile control does not verify identity, models, protocols, quota, or Extra Usage. Every existing admission check remains mandatory. The production Grok adapter remains closed until its interface is verified.')}</p>
          </section>;
        })}
        {snapshot.providers.length === 0 && <EmptyState icon={<Server />} title={t('No providers yet')} body={t('Add a provider or import from CC Switch or Termany.')} />}
        {providers.filter((provider) => isSubscriptionProvider(provider)).map((provider) => {
          const entries = subscriptionCatalog(subscriptionView(snapshot, provider.id));
          if (!entries.length) return null;
          return <section className="provider-catalog" key={provider.id} data-testid={`sub-catalog-rows-${provider.id}`} aria-label={t('{provider} discovered models', { provider: provider.name })}>
            <header><strong>{provider.name}</strong><span>{t('{count} discovered models', { count: entries.length })}</span></header>
            <ul className="provider-catalog-list">
              {entries.map((entry) => {
                const key = `${provider.id}:${entry.internal_id}`;
                const model = catalogModelRow(snapshot.models, entry);
                const disabled = catalogBusy[key] === true || !model;
                return <li className="provider-catalog-row" key={entry.internal_id} data-testid={`sub-catalog-entry-${provider.id}-${entry.internal_id}`}>
                  <div className="provider-catalog-identity">
                    <strong>{entry.name || entry.model_id}</strong>
                    <code title={t('Upstream model ID')}>{entry.model_id}</code>
                    <small>{t('Internal ID')}: {entry.internal_id}</small>
                  </div>
                  <div className="provider-catalog-badges">
                    <span className={cx('provider-catalog-badge', entry.availability)}>{availabilityLabel(entry.availability, t)}</span>
                    <span className={cx('provider-catalog-badge', entry.eligibility)}>{eligibilityLabel(entry.eligibility, t)}</span>
                    <span className={cx('provider-catalog-badge', entry.disabled ? 'disabled' : entry.selected ? 'selected' : 'unselected')}>{t(entry.disabled ? 'Disabled' : entry.selected ? 'Selected' : 'Not selected')}</span>
                  </div>
                  <div className="provider-catalog-actions">
                    <button type="button" className="button ghost small" data-testid={`sub-select-${provider.id}-${entry.internal_id}`} aria-pressed={entry.selected} disabled={disabled} onClick={() => void toggleCatalogEntry(provider, entry, { selected: !entry.selected })}>{t(entry.selected ? 'Remove from model list' : 'Add to model list')}</button>
                    <button type="button" className="button ghost small" data-testid={`sub-enable-${provider.id}-${entry.internal_id}`} aria-pressed={!entry.disabled} disabled={disabled} onClick={() => void toggleCatalogEntry(provider, entry, { enabled: entry.disabled })}>{t(entry.disabled ? 'Enable model' : 'Disable model')}</button>
                  </div>
                  <small className="provider-catalog-note">{t('Deselecting keeps direct calls by the original model ID. Disabling blocks every call.')}</small>
                </li>;
              })}
            </ul>
          </section>;
        })}
      </div>
    </div>
  );
}

function ModelTestErrorDialog({ model, detail, onClose, title = 'Model test failed' }: { model: string; detail: string; onClose: () => void; title?: string }) {
  const { t } = usePreferences();
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { const element = dialog.current; element?.showModal(); return () => element?.close(); }, []);
  return <dialog ref={dialog} className="dialog provider-dialog model-test-error-dialog" aria-labelledby="model-test-error-title" aria-describedby="model-test-error-detail" onCancel={onClose} onClick={event => { if (event.target === event.currentTarget) { const bounds = event.currentTarget.getBoundingClientRect(); if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose(); } }}>
    <div className="provider-dialog-header"><div><h2 id="model-test-error-title">{t(title)}</h2><p>{model}</p></div><button className="icon-action" aria-label={t('Close')} onClick={onClose}><X size={20}/></button></div>
    <div id="model-test-error-detail" className="model-test-error-detail">{detail}</div>
    <div className="provider-dialog-actions"><button autoFocus className="button primary" onClick={onClose}>{t('Close')}</button></div>
  </dialog>;
}

function ModelsPage({ snapshot, onAdd, onEdit, onDelete, onChange, onNotify, onRefreshBudgetSnapshot, speedTests }: { onNotify: (message: string, error?: boolean) => void; onChange: (snapshot: DashboardSnapshot) => void; onRefreshBudgetSnapshot: () => Promise<DashboardSnapshot>; speedTests: ModelSpeedTests; snapshot: DashboardSnapshot; onAdd: () => void; onEdit: (m: Model) => void; onDelete: (id: string) => void }) {
  const { t } = usePreferences();
  const [health, setHealth] = useState(snapshot.health ?? []);
  const [clearingCooldown, setClearingCooldown] = useState<Record<string, boolean>>({});
  const healthEpoch = useRef(0);
  useEffect(() => {
    let stopped = false, pending = false;
    const refresh = async () => {
      if (pending) return;
      pending = true;
      const epoch = healthEpoch.current;
      try { const next = await getGatewayHealth(); if (!stopped && epoch === healthEpoch.current) setHealth(next); }
      catch { /* Retain the last known state until the next refresh. */ }
      finally { pending = false; }
    };
    void refresh();
    const timer = setInterval(() => void refresh(), 3000);
    return () => { stopped = true; clearInterval(timer); };
  }, []);
  const clearCooldown = async (id: string) => {
    healthEpoch.current++;
    setClearingCooldown(previous => ({ ...previous, [id]: true }));
    try {
      const next = await resetGatewayHealth(id);
      healthEpoch.current++;
      setHealth(next.health ?? []); onChange(next); onNotify(t('Cooldown cleared'));
    } catch (error) { onNotify(t(String(error instanceof Error ? error.message : error)), true); }
    finally { setClearingCooldown(previous => ({ ...previous, [id]: false })); }
  };
  const [dismissedSpeedJob, setDismissedSpeedJob] = useState(false);
  useEffect(() => { if (speedTests.view?.job.running) setDismissedSpeedJob(false); }, [speedTests.view?.job.running]);
  const speedError = speedTests.error || (!speedTests.view?.job.running && !dismissedSpeedJob ? speedTests.view?.job.error : '');
  useEffect(() => {
    if (!speedError) return;
    onNotify(t(speedError), true);
    speedTests.dismissError();
    setDismissedSpeedJob(true);
  }, [speedError, onNotify, t, speedTests.dismissError]);
  // 手选池保留所有已建档行；选择、停用、资格与协议能力分别呈现。
  const pool = snapshot.models;
  const testable = speedTestCandidates(snapshot.models, snapshot.providers);
  const selectedCount = testable.filter(model => speedTests.selected.includes(model.id)).length;
  const [tests, setTests] = useState<Record<string, ProviderTestStatus>>({});
  const [toggling, setToggling] = useState<Record<string, boolean>>({});
  const toggle = async (model: Model) => {
    setToggling(previous => ({ ...previous, [model.id]: true }));
    try { onChange(await saveModel({ ...model, enabled: !model.enabled })); onNotify(t(model.enabled ? 'Model disabled' : 'Model enabled')); }
    catch (error) { onNotify(t(String(error)), true); }
    finally { setToggling(previous => ({ ...previous, [model.id]: false })); }
  };
  const selectInPool = async (model: Model, selected: boolean) => {
    setToggling(previous => ({ ...previous, [model.id]: true }));
    try {
      if (snapshot.cpa_subscriptions?.some(connection => connection.provider_id === model.provider_id)) {
        onChange(await selectCpaModel(model.provider_id, model.id, selected));
      } else {
        onChange(await saveModel({ ...model, selected }));
      }
      onNotify(t(selected ? 'Model added to the model list' : 'Model removed from the model list'));
    } catch (error) { onNotify(t(String(error instanceof Error ? error.message : error)), true); }
    finally { setToggling(previous => ({ ...previous, [model.id]: false })); }
  };
  const test = async (model: Model) => {
    const provider = snapshot.providers.find(p => p.id === model.provider_id);
    if (!provider) return;
    setTests(previous => ({ ...previous, [model.id]: 'testing' }));
    try {
      const result = await testProviderDraft({ ...provider, api_type: isSubscriptionProvider(provider) ? '' : model.api_type || provider.api_type, test_model: model.model_id });
      setTests(previous => ({ ...previous, [model.id]: 'success' }));
      onNotify(t(result));
    } catch (error) {
      setTests(previous => ({ ...previous, [model.id]: 'error' }));
      onNotify(providerTestError(error, t), true);
    } finally {
      try { onChange(await onRefreshBudgetSnapshot()); } catch { /* Keep the test result visible. */ }
    }
  };
  return (
    <div className="stack lg">
      <PageIntro title={t("Models")} body={t("Review every discovered source target here. Selection adds it to the pool; disabling blocks calls. Qualification and protocol capability remain separate evidence.")} action={<div className="model-speed-actions"><SpeedTestToolbar tests={speedTests} models={testable}/><button className="button primary" onClick={onAdd}><Plus size={16} />  {t("Add model")}</button></div>} />
      <div className="table-panel provider-table-panel">
        <table className="provider-table models-table">
        <thead><tr><th className="model-speed-check"><label className="model-selection"><input autoComplete="off" autoCapitalize="none" ref={element => { if (element) element.indeterminate = selectedCount > 0 && selectedCount < testable.length; }} type="checkbox" aria-label={t("Select all enabled models")} disabled={!testable.length} checked={testable.length > 0 && selectedCount === testable.length} onChange={e => speedTests.select(e.target.checked ? testable.map(m => m.id) : [])}/></label></th><th>{t("MODEL")}</th><th>{t("Model pool")}</th><th>{t("Enabled status")}</th><th title={t('Time until the first content arrives. Lower is faster.') + ' ' + t('Output tokens per second after the first content. Higher is faster.')}>{t('First response / Output speed')}</th><th>{t("INPUT / OUTPUT")}</th><th>{t("Model information")}</th><th className="provider-actions-heading">{t("Actions")}</th></tr></thead>
        <tbody>
        {pool.map((model) => {
          const provider = snapshot.providers.find(p => p.id === model.provider_id);
          const cpa = snapshot.cpa_subscriptions?.find(connection => connection.provider_id === model.provider_id);
          const cpaModel = cpa?.models.find(candidate => candidate.id === model.id);
          const apiSource = snapshot.api_sources?.[model.provider_id];
          const modelHealth = health.find(h => h.model_id === model.id && h.state !== 'healthy');
          const subscription = isSubscriptionProvider(provider) ? subscriptionView(snapshot, model.provider_id) : undefined;
          const capability = cpa ? (cpa.capability as 'unverified'|'verified'|'unsupported') : subscription ? modelCapability(subscription, model.model_id, protocolKey(model.api_type || provider?.api_type || 'chat_completions')) : 'unverified';
          const catalogEntry = subscription?.catalog_entries?.find(entry => entry.internal_id === model.id);
          const protocol = protocolKey(model.api_type || provider?.api_type || 'chat_completions');
          const catalogState = cpa ? (cpa.catalog_state === 'stale' ? 'Stale (last confirmed read kept)' : cpa.catalog_state === 'available' ? 'Catalog available' : 'Catalog unknown') : subscription?.catalog?.state ? catalogLabel(subscription.catalog.state, t) : 'Catalog unknown';
          const qualification = catalogEntry ? eligibilityLabel(catalogEntry.eligibility, t) : cpa ? (cpa.qualification === 'unknown' ? 'Unknown' : cpa.qualification) : subscription ? 'Unknown' : 'Not applicable (API credential target)';
          const account = cpa ? `${cpa.account ?? t('Unknown')} (CPA)` : subscription ? identityLabel(subscription, t) : apiSource?.account_label ? `${apiSource.account_label} (${apiSource.account_state === 'user_declared' ? 'user declared' : 'unknown'})` : t('Unknown');
          const plan = cpa ? `${cpa.plan ?? t('Unknown')} (CPA)` : subscription ? (subscription.quota.buckets?.find(bucket => bucket.plan_type)?.plan_type ?? t('Unknown')) : apiSource?.plan_label ? `${apiSource.plan_label} (${apiSource.plan_state === 'user_declared' ? 'user declared' : 'unknown'})` : t('Unknown');
          const sourceKind = apiSource ? (apiSource.kind === 'official_api' ? 'Official API' : apiSource.kind === 'third_party_api' ? 'Third-party API' : 'Coding Plan (Key)') : cpa ? 'CPA subscription' : provider?.kind === 'codex_subscription' || provider?.kind === 'grok_subscription' ? 'Subscription' : provider?.kind === 'ollama' ? 'Local API' : 'Legacy API';
          const protocolName = protocol === 'responses' ? 'Responses' : protocol === 'messages' ? 'Messages' : 'Chat Completions';
          return (
          <tr key={model.id} className={speedTests.selected.includes(model.id) ? 'is-selected' : undefined} data-testid={`model-pool-row-${model.id}`} data-model-id={model.id} data-model-bound={cpaModel?.bound ?? true}>
            <td className="model-speed-check"><label className="model-selection"><input autoComplete="off" autoCapitalize="none" type="checkbox" aria-label={t("Select model for speed test") + ": " + model.provider_id + "/" + model.model_id} disabled={!testable.some(m => m.id === model.id)} checked={speedTests.selected.includes(model.id)} onChange={() => speedTests.toggle(model.id)}/></label></td>
            <td><div className="provider-table-name"><span className="provider-table-icon"><ProviderLogo id={provider ? providerPreset(provider) : 'custom-openai'}/></span><div><strong>{model.name || model.model_id}</strong><small>{sourceKind} · {provider?.name ?? model.provider_id}</small><small>来源/连接 ID：{provider ? providerIdentifier(provider) : model.provider_id} · {apiSource ? `实例 ${apiSource.connection_instance_id} / 世代 ${apiSource.generation}` : cpa ? `实例 ${cpa.connection_instance_id} / 世代 ${cpa.generation}` : '连接身份未知'}</small><small>账号：{account} · 套餐：{plan}</small><small>上游模型 ID：{model.model_id}</small><small>稳定调用 ID：<code>autojev/model/{model.id}</code></small></div></div></td>
            <td><div className="provider-enabled-cell"><button type="button" role="switch" data-testid={`model-pool-select-${model.id}`} aria-checked={modelSelected(model)} aria-label={t('Select model {model}', { model: model.name || model.model_id })} disabled={toggling[model.id] || (!!cpaModel && !cpaModel.bound && !modelSelected(model))} className={cx('switch', modelSelected(model) && 'on')} onClick={() => void selectInPool(model, !modelSelected(model))}><span/></button><span>{t(modelSelected(model) ? 'SELECTED' : 'NOT SELECTED')}</span></div></td>
            <td><div className="provider-enabled-cell"><button type="button" role="switch" aria-checked={model.enabled} aria-label={t('Enable {provider}', { provider: model.name })} disabled={toggling[model.id]} className={cx('switch', model.enabled && 'on')} onClick={() => void toggle(model)}><span/></button><span>{t(model.enabled ? 'ENABLED' : 'DISABLED')}</span></div>{modelHealth && <div className="model-health-status" title={modelHealth.last_status ? `HTTP ${modelHealth.last_status}` : t('Connection failed')}><span>{t(modelHealth.state === 'cooldown' ? 'Cooling down' : modelHealth.state === 'probing' ? 'Checking recovery' : 'Awaiting recovery')}{modelHealth.retry_after_seconds > 0 ? ` · ${modelHealth.retry_after_seconds}s` : ''}</span><button type="button" className="icon-action" title={t('Clear cooldown')} aria-label={t('Clear cooldown') + ': ' + (model.name || model.model_id)} disabled={clearingCooldown[model.id]} onClick={() => void clearCooldown(model.id)}><RefreshCw size={13} className={clearingCooldown[model.id] ? 'import-spinner' : ''}/></button></div>}</td>
            <td><SpeedCell result={speedTests.view?.models[model.id]} running={speedTests.view?.job.running === true && speedTests.view.job.current_models.includes(model.id)}/></td>
            <td><code className="provider-table-url">{(model.input_price_known ?? model.input_cost_per_million > 0) ? `$${model.input_cost_per_million}` : t('Unknown')} / {(model.output_price_known ?? model.output_cost_per_million > 0) ? `$${model.output_cost_per_million}` : t('Unknown')}</code></td>
            <td><div className="model-info-cell"><span className={model.supports_vision ? 'model-image-supported' : 'model-image-unsupported'} role="img" aria-label={t(model.supports_vision ? 'Supports image input' : 'Image input not marked as supported')} title={t(model.supports_vision ? 'Supports image input' : 'Image input not marked as supported')}>{model.supports_vision ? <ImageIcon size={16} aria-hidden="true"/> : <ImageOff size={16} aria-hidden="true"/>}</span><span title={t('Context length (tokens)') + ': ' + (model.context_window > 0 ? model.context_window.toLocaleString() : t('Unknown'))}>{model.context_window > 0 ? new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 2 }).format(model.context_window) : '—'}</span><div className="model-pool-evidence"><span data-evidence="catalog">目录：{catalogState}</span><span data-evidence="qualification">资格：{qualification}</span><span data-evidence="protocol">协议能力：{capabilityLabel(capability, t)} · 配置 {protocolName}{!subscription && !cpa ? '（未独立验证）' : ''}</span></div></div></td>
            <td><div className="row-actions">{cpaModel && !cpaModel.bound && <button type="button" className="button ghost small" data-testid={`model-pool-rebind-${model.id}`} disabled={toggling[model.id]||cpa?.stage!=='connected'||cpa.catalog_state!=='available'} onClick={() => void selectInPool(model, true)}>重新绑定当前账号并加入模型池</button>}<button className="icon-action" disabled={!provider || tests[model.id] === 'testing'} title={t('Test')} aria-label={t('Test')} onClick={() => void test(model)}>{tests[model.id] === 'testing' ? <LoaderCircle size={15} className="import-spinner"/> : <Play size={15}/>}</button><button className="icon-action" data-testid={`model-configure-${model.id}`} title={t("Configure")} aria-label={t("Configure")} onClick={() => onEdit(model)}><Settings2 size={15} /></button><button className="icon-action danger" title={t("Delete")} aria-label={t("Delete")} onClick={() => onDelete(model.id)}><Trash2 size={15} /></button></div></td>
          </tr>
        ); })}
        </tbody></table>
        {pool.length === 0 && <EmptyState icon={<Cpu />} title={t("No models in the pool")} body={t("Add at least one model before starting the proxy.")} />}
      </div>
    </div>
  );
}

function AgentsPage({ snapshot, onRefresh, onConnect, onRestore, onChange, onManageRoutes, connecting }: { connecting: boolean; onManageRoutes: () => void; onChange: (snapshot: DashboardSnapshot) => void; snapshot: DashboardSnapshot; onRefresh: () => Promise<void>; onConnect: (id: string, routeId: string, routeIds: string[]) => Promise<void>; onRestore: (id: string) => Promise<void> }) {
  const { t } = usePreferences();
  const [operationError, setOperationError] = useState<{name: string; detail: string} | null>(null);
  const [activeAgent, setActiveAgent] = useState<string | null>(null);
  const batchLock = useRef(false);
  const [batchProgress, setBatchProgress] = useState<{ done: number; total: number } | null>(null);
  const [batchResult, setBatchResult] = useState<{ succeeded: string[]; failed: { name: string; error: string }[] } | null>(null);
  const operate = async (id: string, name: string, work: () => Promise<void>) => {
    if (activeAgent) return;
    setActiveAgent(id); setOperationError(null);
    try { await work(); }
    catch (e) { setOperationError({name, detail: t(e instanceof Error ? e.message : String(e))}); }
    finally { setActiveAgent(null); }
  };
  const [selections, setSelections] = useState<Record<string, string[]>>(() => {
    try { return parseAgentSelections(localStorage.getItem(AGENT_SELECTIONS_KEY)); }
    catch { return {}; }
  });
  const [detecting, setDetecting] = useState(false);
  const [error, setError] = useState('');
  const [adding, setAdding] = useState(false);
  const [editingCustom, setEditingCustom] = useState<DashboardSnapshot['agents'][number] | undefined>();
  useEffect(() => {
    try { localStorage.setItem(AGENT_SELECTIONS_KEY, JSON.stringify(selections)); }
    catch { setError(t('Could not save model and route selections.')); }
  }, [selections, t]);
  const agents = snapshot.agents.filter(agent => agent.custom || (agent.installed && agent.can_connect !== false));
  // 路由可用性与后端连接校验同一套规则：作用域（all_models 只自动包含 API 模型）+ 选择 + 停用 + 服务商 + 协议。
  const availableRoutes = connectableRoutes(snapshot.routes, snapshot.models, snapshot.providers).sort((a, b) => a.id.localeCompare(b.id, 'en'));
  const detect = async () => {
    setDetecting(true); setError('');
    try { await onRefresh(); } catch { setError(t('Could not detect agents. Try again.')); }
    finally { setDetecting(false); }
  };
  // Agent 可选模型与测速候选同源：未选或停用的模型都不进注入目录。
  const selectableModels = agentSelectableModels(snapshot.models, snapshot.providers);
  const availableBindings = new Set([
    ...availableRoutes.map(route => route.id),
    ...selectableModels.map(model => 'model/' + model.id),
  ]);
  const savedSelections = (agent: DashboardSnapshot['agents'][number]) => selections[agent.id] ?? snapshot.agent_catalogs?.[agent.id]?.map(entry => entry.binding) ?? (agent.route_id ? [agent.route_id] : []);
  /** 磁盘上的注入目录（默认项在前）；没有保存过目录时以本地选择为准。 */
  const savedCatalogBindings = (agent: DashboardSnapshot['agents'][number]) => snapshot.agent_catalogs?.[agent.id]?.map(entry => entry.binding);
  const batchTargets = snapshot.agents.map(agent => ({ agent, chosen: availableAgentSelections(savedSelections(agent), availableBindings) }))
    .filter(({ agent, chosen }) => agent.connected && agent.installed && agent.can_connect !== false && chosen.length > 0);
  const injectAll = async () => {
    if (batchLock.current || connecting || activeAgent || detecting || !batchTargets.length) return;
    batchLock.current = true;
    setActiveAgent('batch'); setOperationError(null); setBatchResult(null);
    const targets = batchTargets.map(({ agent, chosen }) => ({ agent, chosen: [...chosen] }));
    const result: { succeeded: string[]; failed: { name: string; error: string }[] } = { succeeded: [], failed: [] };
    setBatchProgress({ done: 0, total: targets.length });
    try {
      for (const [index, { agent, chosen }] of targets.entries()) {
        try {
          onChange(await connectAgent(agent.id, chosen[0], chosen, true));
          setSelections(previous => ({ ...previous, [agent.id]: chosen }));
          result.succeeded.push(agent.name);
        } catch (e) { result.failed.push({ name: agent.name, error: t(e instanceof Error ? e.message : String(e)) }); }
        setBatchProgress({ done: index + 1, total: targets.length });
      }
      setBatchResult(result);
    } finally { batchLock.current = false; setBatchProgress(null); setActiveAgent(null); }
  };
  return <div className="stack lg">
    {operationError && <ModelTestErrorDialog title="Agent operation failed" model={operationError.name} detail={operationError.detail} onClose={() => setOperationError(null)}/>}
    <PageIntro title={t('Agents')} body={t('Detected local agents. Select a route to connect to AutoJev.')} action={<div className="provider-actions agent-page-actions"><button className="button ghost" disabled={detecting || connecting || activeAgent !== null} onClick={() => void detect()}><RefreshCw size={16} className={detecting ? 'import-spinner' : ''}/>{t(detecting ? 'Detecting…' : 'Detect again')}</button><button className="button ghost" disabled={connecting || detecting || activeAgent !== null || !batchTargets.length} title={t('Update configurations for connected agents only. Disconnected agents remain disconnected.')} onClick={() => void injectAll()}>{batchProgress ? <LoaderCircle size={16} className="import-spinner"/> : <Plug size={16}/>} {batchProgress ? t('Updating {done}/{total}', batchProgress) : t('Batch update configurations')}</button><button className="button primary" disabled={connecting || activeAgent !== null} onClick={() => { setError(''); setEditingCustom(undefined); setAdding(true); }}><Plus size={16}/>{t('Add agent')}</button></div>} />
    <div className="agent-list-toolbar"><div><button className="text-button" onClick={onManageRoutes}><GitBranch size={16}/>{t('Manage routes')}</button>{!availableRoutes.length && <p>{t('Create a route to use multiple models, or select a model directly.')}</p>}</div></div>
    {batchResult && <div className="agent-batch-result" role="status">
      <div><strong>{t('Update complete: {success} succeeded, {failed} failed', { success: batchResult.succeeded.length, failed: batchResult.failed.length })}</strong><button className="icon-action" aria-label={t('Close')} onClick={() => setBatchResult(null)}><X size={15}/></button></div>
      {!!batchResult.succeeded.length && <p>{batchResult.succeeded.join(' · ')}<br/>{t('Restart the corresponding clients to refresh their model lists.')}</p>}
      {batchResult.failed.map(item => <p className="route-error" key={item.name}>{item.name}: {item.error}</p>)}
    </div>}
    {error && <p className="route-error" role="alert">{error}</p>}
    <div className="table-panel provider-table-panel agent-table-panel">
      <table className="provider-table agent-table">
        <thead><tr><th>{t('Agent')}</th><th>{t('Configuration file')}</th><th>{t('Status')}</th><th>{t('Model / route')}</th><th className="provider-actions-heading">{t('Actions')}</th></tr></thead>
        <tbody>{agents.map(agent => {
          let selected = agent.route_id ?? '';
          const options = [
            ...availableRoutes.map(route => ({ value: route.id, label: route.id, keywords: route.name, group: t('Routes'), icon: <GitBranch size={18}/> })),
            ...selectableModels.map(model => {
              const provider = snapshot.providers.find(p => p.id === model.provider_id)!;
              return { value: 'model/' + model.id, label: providerIdentifier(provider) + '/' + model.model_id, keywords: model.name + ' ' + provider.name, group: t('Models'), icon: <ProviderLogo id={providerPreset(provider)}/> };
            }).sort((a, b) => a.label.localeCompare(b.label, 'en', { sensitivity: 'base' }))
          ];
          const saved = savedSelections(agent);
          const chosen = availableAgentSelections(saved, availableBindings);
          const unavailable = saved.filter(binding => !availableBindings.has(binding));
          // 保存目录与当前应用内选择不一致即待同步；后端停用/撤销不等待这里。
          const savedCatalog = savedCatalogBindings(agent);
          const pendingSync = agentCatalogPendingSync({ saved: savedCatalog ?? selections[agent.id] ?? [], chosen, pendingSync: agent.catalog_pending_sync });
          selected = chosen[0] ?? '';
          const allSelected = options.length > 0 && options.every(option => chosen.includes(option.value));
          const valid = options.some(option => option.value === selected) && chosen.includes(selected) && chosen.every(id => options.some(o => o.value === id));
          return <tr key={agent.id}>
            <td><div className="provider-table-name"><span className="provider-table-icon"><AgentLogo id={agent.id} icon={agent.icon}/></span><div className="agent-name-details"><strong>{agent.name}</strong><div className="agent-launch-command"><code title={[agent.command, ...(agent.args ?? [])].join(' ')}>{agent.custom ? (agent.command ?? '').split(/[\\/]/).filter(Boolean).pop() : [agent.command, ...(agent.args ?? []).map(arg => /[\s'"$`\\]/.test(arg) ? JSON.stringify(arg) : arg)].join(' ')}</code><button type="button" className="icon-action" disabled={!agent.installed || activeAgent === agent.id} title={t('Open in terminal')} aria-label={t('Open {name} in terminal', { name: agent.name })} onClick={() => void operate(agent.id, agent.name, () => launchAgent(agent.id))}>{activeAgent === agent.id ? <LoaderCircle size={15} className="import-spinner"/> : <Terminal size={15}/>}</button></div>{!agent.custom && agent.can_connect === false && <small title={agent.connection_note ? t(agent.connection_note) : undefined}>{t('Connection unavailable')}</small>}</div></div></td>
            <td><div className="agent-config-paths">{agent.config_path ? agent.config_path.split(', ').map((path, index) => <div className="agent-config-path" key={path}>
              <code className="provider-table-url" title={path}>{path}</code>
              {/\.(?:jsonc?|toml|ya?ml|env|ini|conf|cfg)$/i.test(path) && <button type="button" className="icon-action" title={t('Open configuration in text editor')} aria-label={t('Open configuration in text editor') + ': ' + path} onClick={() => { void openAgentConfig(agent.id, index).catch(e => setOperationError({ name: agent.name, detail: t(String(e)) })); }}><FilePenLine size={15}/></button>}
            </div>) : '—'}</div></td>
            <td><span className={cx('tag', agent.connected ? 'green' : 'muted')}>{t(agent.connected ? 'CONNECTED' : agent.installed ? 'Not connected' : 'NOT FOUND')}</span></td>
            <td><div className="agent-route-select"><SearchSelect value="" values={chosen} bulkActions={<><span className="search-select-total">{t('Total {count} options', { count: options.length })}</span><label className="search-select-all">
              <span className="model-selection"><input type="checkbox" autoComplete="off" autoCapitalize="none" checked={allSelected} ref={node => { if (node) node.indeterminate = chosen.length > 0 && !allSelected; }} onChange={event => {
                const next = event.target.checked ? [...chosen.filter(id => options.some(option => option.value === id)), ...options.filter(option => !chosen.includes(option.value)).map(option => option.value)] : [];
                setSelections(previous => ({ ...previous, [agent.id]: next }));
              }}/></span>{t('Select all')}
            </label></>} selectionControls={<label className="agent-default-selection">
              <span>{t('Default model / route')}</span>
              <select value={selected} disabled={!chosen.length || connecting || activeAgent !== null} onChange={event => {
                const value = event.target.value;
                if (chosen.includes(value)) setSelections(previous => ({ ...previous, [agent.id]: [value, ...chosen.filter(id => id !== value)] }));
              }}>
                {!chosen.length && <option value="">{t('Not selected')}</option>}
                {chosen.map(value => <option key={value} value={value}>{options.find(option => option.value === value)?.label ?? `${t('Unavailable selection')} · ${value}`}</option>)}
              </select>
            </label>} onChange={() => {}} onToggle={value => {
              const next = chosen.includes(value) ? chosen.filter(id => id !== value) : [...chosen, value];
              setSelections(previous => ({ ...previous, [agent.id]: next }));
            }} label={t(chosen.length ? 'Selected models / routes' : 'Not selected')} placeholder={t('Search models or routes…')} empty={t('No matching options')} disabled={connecting || activeAgent !== null || !options.length || agent.can_connect === false} options={options.map(o => o.value === chosen[0] ? { ...o, label: `${o.label} · ${t('Default model')}` } : o)}/>
              {unavailable.length > 0 && <small className="agent-selection-notice" role="status">{t('Unavailable selections excluded: {names}', { names: unavailable.map(binding => snapshot.models.find(model => 'model/' + model.id === binding)?.name ?? snapshot.routes.find(route => route.id === binding)?.name ?? snapshot.agent_catalogs?.[agent.id]?.find(entry => entry.binding === binding)?.name ?? binding).join(' · ') })}{' '}{t(chosen.length ? 'The first remaining selection is the default.' : 'Select a model or route before connecting.')}</small>}
              {pendingSync && <small className="agent-selection-notice agent-pending-sync" role="status" data-testid={`agent-pending-sync-${agent.id}`}>{t('Pending sync: the saved catalog differs from the current selection. Reconnect to update the agent configuration.')}{' '}{t('Removals and revocations apply in the backend immediately; no external configuration refresh is needed.')}</small>}
            </div></td>
            <td><div className="row-actions">{agent.custom && <button className="icon-action" disabled={connecting || activeAgent !== null || agent.connected} title={t(agent.connected ? 'Disconnect the agent before editing its configuration' : 'Configure')} aria-label={t('Configure')} onClick={() => { setEditingCustom(agent); setAdding(true); }}><Settings2 size={15}/></button>}<button className="icon-action" disabled={connecting || activeAgent !== null} onClick={() => {
              const reason = !agent.installed ? 'Agent executable was not found. Detect again after installing it.' : agent.can_connect === false ? (agent.connection_note || 'Connection unavailable') : !selected ? 'Select a model or route before connecting.' : !valid ? 'Some selected models or routes are unavailable. Remove the unavailable selections or enable their models and providers.' : '';
              if (reason) { setOperationError({ name: agent.name, detail: t(reason) }); return; }
              void operate(agent.id, agent.name, async () => { await onConnect(agent.id, selected, chosen); setSelections(previous => ({ ...previous, [agent.id]: chosen })); });
            }} title={t(agent.connected ? 'Reconnect to AutoJev' : 'Connect to AutoJev')} aria-label={t(agent.connected ? 'Reconnect to AutoJev' : 'Connect to AutoJev')}>{activeAgent === agent.id ? <LoaderCircle size={16} className="import-spinner"/> : agent.connected ? <RefreshCw size={16}/> : <Plug size={16}/>}</button>{agent.connected && <button className="icon-action" disabled={connecting || activeAgent !== null} onClick={() => void operate(agent.id, agent.name, () => onRestore(agent.id))} title={t('Disconnect and restore original configuration')} aria-label={t('Disconnect and restore original configuration')}><Unplug size={16}/></button>}</div></td>
          </tr>;
        })}</tbody>
      </table>
      {!agents.length && <EmptyState icon={<Bot/>} title={t('No supported agents detected')} body={t('Install a supported agent and detect again, or add one manually.')} />}
    </div>
    {adding && <AddAgentDialog initial={editingCustom} onClose={() => setAdding(false)} onSaved={onChange}/>}
  </div>;
}

function AgentLogo({ id, icon }: { id: string; icon?: string | null }) {
  const [failed, setFailed] = useState(false);
  if (icon && !failed) return <img className="agent-brand-logo" src={icon} alt="" onError={() => setFailed(true)}/>;
  if (id.startsWith('custom-') || failed) return <Bot size={20}/>;
  const extension = id === 'fastclaw' ? 'png' : id === 'hermes' ? 'webp' : 'svg';
  return <img className={cx('agent-brand-logo', id === 'kimi' && 'kimi-brand-logo', ['cursor', 'opencode', 'grok'].includes(id) && 'monochrome-logo')} src={`/icons/agents/${id}${['gemini', 'claude', 'codex', 'kimi', 'openclaw'].includes(id) ? '-color' : ''}.${extension}`} alt="" onError={() => setFailed(true)}/>;
}

function EventList({ events, detailed }: { events: DashboardSnapshot['events']; detailed?: boolean }) {
  const { t } = usePreferences();
  if (!events.length) return <EmptyState icon={<Activity />} title={t("No routes yet")} body={t("Connect an agent or use the route preview to generate the first decision.")} />;
  return <div className="event-list">{events.map((event) => <div className="event-row" key={event.id}><span className={cx('event-icon', event.source)}>{event.source === 'jev' ? <Sparkles size={15} /> : <GitBranch size={15} />}</span><div className="event-main"><strong>{event.model_name}</strong><span>{event.endpoint} · {event.provider_name}{detailed ? ` · ${event.reason}` : ''}</span></div>{detailed && <code>{event.estimated_input_tokens.toLocaleString()} tok</code>}<div className="event-cost"><strong>{money(event.estimated_cost)}</strong><span className="positive">−{money(event.estimated_savings)}</span></div><time>{timeAgo(event.created_at, t)}</time></div>)}</div>;
}

const PROVIDER_PRESETS = [
  { id: 'openai', name: 'OpenAI', kind: 'openai_compatible', url: 'https://api.openai.com/v1' },
  { id: 'deepseek', name: 'DeepSeek', kind: 'openai_compatible', url: 'https://api.deepseek.com/v1' },
  { id: 'openrouter', name: 'OpenRouter', kind: 'openrouter', url: 'https://openrouter.ai/api/v1' },
  { id: 'zenmux', name: 'ZenMux', kind: 'openai_compatible', url: '' },
  { id: 'ollama', name: 'Ollama', kind: 'ollama', url: 'http://127.0.0.1:11434' },
  { id: 'custom-openai', name: 'OpenAI Compatible', kind: 'openai_compatible', url: '' },
  { id: 'custom-anthropic', name: 'Anthropic Compatible', kind: 'openai_compatible', url: '' },
  { id: 'codex-subscription', name: 'Codex subscription', kind: 'codex_subscription', url: '' },
  { id: 'grok-subscription', name: 'Grok subscription', kind: 'grok_subscription', url: '' },
] as const;

function providerPreset(provider: Provider) {
  const preset = PROVIDER_PRESETS.find((p) => p.id === provider.preset);
  if (preset) return preset.id;
  if (provider.kind === 'codex_subscription') return 'codex-subscription';
  if (provider.kind === 'grok_subscription') return 'grok-subscription';
  let host = '';
  try { host = new URL(provider.base_url).hostname; } catch { /* Use protocol fallback. */ }
  if (host === 'api.deepseek.com') return 'deepseek';
  if (host === 'api.openai.com') return 'openai';
  if (host === 'openrouter.ai' || provider.kind === 'openrouter') return 'openrouter';
  if (provider.kind === 'ollama') return 'ollama';
  return provider.api_type === 'messages' ? 'custom-anthropic' : 'custom-openai';
}

function ProviderLogo({ id }: { id: string }) {
  if (id === 'zenmux' || id.startsWith('custom-') || id.endsWith('-subscription')) return <Server size={18} aria-hidden="true" />;
  return <img alt="" className={cx('provider-logo', ['openai', 'ollama'].includes(id) && 'monochrome-logo')} src={`/icons/providers/${id}${['deepseek', 'anthropic', 'openrouter'].includes(id) ? '-color' : ''}.svg`} />;
}

function normalizeApiType(value?: string) {
  return value === 'responses' || value === 'messages' ? value : 'chat_completions';
}

function ProviderDialog({ initial, initialSource, subscriptionTargets, onClose, onSave, onTestStatus }: { onTestStatus: (status: ProviderTestStatus) => void | Promise<void>; initial: Provider; initialSource?: ApiSourceDraft; subscriptionTargets: SubscriptionCatalogEntry[]; onClose: () => void; onSave: (p: Provider, key?: string, addTestModel?: boolean, source?: ApiSourceDraft) => Promise<void> }) {
  const { t } = usePreferences();
  const edit = Boolean(initial.id);
  const inferred = PROVIDER_PRESETS.find((p) => p.id === initial.preset)?.id
    || (initial.kind === 'codex_subscription' ? 'codex-subscription' : initial.kind === 'grok_subscription' ? 'grok-subscription' : undefined)
    || PROVIDER_PRESETS.find((p) => p.url && initial.base_url.startsWith(p.url))?.id
    || (initial.api_type === 'messages' ? 'custom-anthropic' : 'custom-openai');
  const [provider, setProvider] = useState(() => ({ ...initial, id: initial.id || crypto.randomUUID(), name: initial.name || (inferred.startsWith('custom-') ? 'Custom' : PROVIDER_PRESETS.find((p) => p.id === inferred)?.name) || '', preset: inferred, api_type: normalizeApiType(initial.api_type), test_model: initial.test_model || '' }));
  const [identifier, setIdentifier] = useState(initial.id || (inferred.startsWith('custom-') ? 'custom' : inferred));
  const [source, setSource] = useState<ApiSourceDraft | undefined>(() => initialSource || (!edit && !inferred.startsWith('custom-') && initial.kind !== 'ollama' && !isSubscriptionKind(initial.kind) ? { kind: inferred === 'openrouter' || inferred === 'zenmux' ? 'third_party_api' : 'official_api' } : undefined));
  const [addSourceModel, setAddSourceModel] = useState(true);
  const lockedSource = Boolean(initialSource);
  const [apiKey, setApiKey] = useState('');
  const [showKey, setShowKey] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState('');
  const [testFailed, setTestFailed] = useState(false);
  const [testedFingerprint, setTestedFingerprint] = useState('');
  const fingerprint = () => JSON.stringify([provider.base_url.trim(), provider.api_type, provider.kind, provider.test_model.trim(), apiKey]);
  const form = useRef<HTMLFormElement>(null);
  const subscription = isSubscriptionKind(provider.kind);
  const providerTypeChanged = edit && provider.kind !== initial.kind;
  const availableSubscriptionTargets = providerTypeChanged ? [] : subscriptionTargets;
  const changePreset = (id: string) => {
    const preset = PROVIDER_PRESETS.find((p) => p.id === id)!;
    const kind = preset.kind as Provider['kind'];
    const subscriptionPreset = isSubscriptionKind(kind);
    if (!edit) setSource(subscriptionPreset || kind === 'ollama' || id.startsWith('custom-') ? undefined : { kind: id === 'openrouter' || id === 'zenmux' ? 'third_party_api' : 'official_api' });
    setProvider({ ...provider, preset: preset.id, kind, base_url: preset.url, name: id.startsWith('custom-') ? 'Custom' : preset.name, api_type: subscriptionPreset ? '' : id === 'custom-anthropic' ? 'messages' : 'chat_completions', test_model: '' });
    if (!edit) setIdentifier(id.startsWith('custom-') ? 'custom' : id);
    // 订阅条目没有密钥字段：切到订阅时丢弃已输入的密钥，避免隐藏字段的残留值随保存提交。
    if (subscriptionPreset) { setApiKey(''); setShowKey(false); }
    setTestResult('');
  };
  const draft = (): Provider => {
    const next = { ...provider, id: identifier.trim(), name: provider.name.trim() || identifier.trim() };
    // 订阅服务商没有 base URL 与 API 类型；测试目标是用户从已发现目录选择的上游模型 ID。
    return isSubscriptionKind(next.kind) ? { ...next, base_url: '', api_type: '' } : next;
  };
  const submit = async (event: FormEvent) => {
    event.preventDefault(); if (testing) return;
    setTesting(true); setTestResult('');
    const next = draft();
    try { await onSave(next, isSubscriptionKind(next.kind) ? undefined : apiKey || undefined, source ? !edit && addSourceModel && Boolean(next.test_model?.trim()) : !edit && testedFingerprint === fingerprint(), source); setApiKey(''); }
    catch (error) { setTestFailed(true); setTestResult(t(String(error instanceof Error ? error.message : error))); }
    finally { setTesting(false); }
  };
  const test = async () => {
    if (testing || !form.current?.reportValidity()) return;
    if (subscription && providerTypeChanged) {
      setTestFailed(true); setTestResult(t('Save this provider type and refresh its model catalog before testing.')); await onTestStatus('error'); return;
    }
    if (!provider.test_model.trim()) { setTestFailed(true); setTestResult(t('Enter a test model')); await onTestStatus('error'); return; }
    setTestedFingerprint(''); setTesting(true); setTestResult(''); setTestFailed(false); await onTestStatus('testing');
    try { const result = await testProviderDraft({ ...draft(), id: initial.id || draft().id }, apiKey || undefined, source); setTestedFingerprint(fingerprint()); if (!edit) setProvider(previous => ({ ...previous, enabled: true })); setTestResult(t(result)); await onTestStatus('success'); }
    catch (error) {
      setTestFailed(true);
      setTestResult(providerTestError(error, t));
      await onTestStatus('error');
    }
    finally { setTesting(false); }
  };
  let base = provider.base_url.trim().replace(/\/+$/, '');
  if (!source && !base.endsWith('/v1')) base += '/v1';
  const endpoint = `${base}/${provider.api_type === 'messages' ? 'messages' : provider.api_type === 'responses' ? 'responses' : 'chat/completions'}`;
  return <div className="modal-backdrop" onMouseDown={(event) => event.target === event.currentTarget && !testing && onClose()}>
    <div className="dialog provider-dialog" role="dialog" aria-modal="true" aria-labelledby="provider-dialog-title">
      <div className="provider-dialog-header"><div><h2 id="provider-dialog-title">{t(edit ? 'Edit provider' : 'Create provider')}</h2><p>{t(subscription ? 'Add a subscription provider to connect a Codex or Grok account.' : edit ? 'Update provider configuration.' : 'Add a new upstream API provider.')}</p></div><button type="button" className="icon-action" disabled={testing} onClick={onClose} aria-label={t('Close')}><X size={20} /></button></div>
      <form autoComplete="off" ref={form} onSubmit={submit} className="provider-dialog-form">
        <fieldset disabled={testing}>
          <div className="field-pair">
            <div className="form-field"><span>{t('Provider type')}</span><SearchSelect value={provider.preset} disabled={testing || lockedSource} label={t('Provider type')} placeholder={t('Search providers…')} empty={t('No providers found')} onChange={changePreset} options={[...PROVIDER_PRESETS.filter((p) => !p.id.startsWith('custom-')).sort((a, b) => a.name.localeCompare(b.name, 'en')), ...PROVIDER_PRESETS.filter((p) => p.id.startsWith('custom-'))].map((p) => ({ value: p.id, label: t(p.name), icon: <ProviderLogo id={p.id} />, group: p.id.startsWith('custom-') ? t('Custom') : t('Recommended'), keywords: `${p.name} ${p.id} ${p.url}` }))} /></div>
            {!subscription && <label className="form-field"><span>{t('API type')}</span><Select disabled={lockedSource} searchable={false} value={provider.api_type} onChange={(e) => { setProvider({ ...provider, api_type: e.target.value }); setTestResult(''); }}><option value="chat_completions">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="messages">Anthropic Messages</option></Select></label>}
          </div>
          {!subscription && provider.kind !== 'ollama' && (!edit || lockedSource) && <ApiSourceFields source={source} locked={lockedSource} onChange={setSource}/>}
          <div className="field-pair">
            <label className="form-field"><span>{t('Provider name')}</span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} required pattern="[a-zA-Z0-9_-]+" disabled={lockedSource} value={identifier} onChange={(e) => setIdentifier(e.target.value)} /></label>
            <label className="form-field"><span>{t('Display name')} <small>{t('(optional)')}</small></span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} value={provider.name} placeholder={identifier} onChange={(e) => setProvider({ ...provider, name: e.target.value })} /></label>
          </div>
          {!subscription && <label className="form-field"><span>{t(source ? 'Source protocol base URL' : 'Base URL')}</span><input disabled={lockedSource} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} required type="url" value={provider.base_url} placeholder="https://api.example.com/v1" onChange={(e) => setProvider({ ...provider, base_url: e.target.value })} /></label>}
          {!subscription && <label className="form-field"><span>{t(source ? 'Generation API key' : 'API key')}</span><div className="secret-input"><input disabled={lockedSource} autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} value={apiKey} type={showKey ? 'text' : 'password'} placeholder={provider.has_api_key ? t('Leave blank to keep the stored credential.') : provider.kind === 'ollama' ? t('No API key required') : 'sk-…'} onChange={(e) => setApiKey(e.target.value)} /><button type="button" aria-label={t('API key')} onClick={() => setShowKey(!showKey)}>{showKey ? <EyeOff size={16} /> : <Eye size={16} />}</button></div></label>}
          {subscription && <div className="form-field subscription-provider-note"><ShieldCheck size={16} aria-hidden="true" /><p className="provider-test-help">{t('Subscription providers keep authorization in the official helper managed by this app. Generation stays denied until identity, capability and quota are verified.')}</p></div>}
          {subscription && <div className="form-field" data-testid="provider-subscription-test-field"><label htmlFor="provider-subscription-test-model">{t('Test model')}</label><select id="provider-subscription-test-model" data-testid="provider-subscription-test-model" value={provider.test_model || ''} disabled={testing || !edit || providerTypeChanged || (!availableSubscriptionTargets.length && !provider.test_model)} onChange={(event) => { setProvider({ ...provider, test_model: event.target.value }); setTestResult(''); }}><option value="">{providerTypeChanged ? t('Save this provider type and refresh its model catalog before testing.') : availableSubscriptionTargets.length ? t('Choose a discovered model') : t('Refresh the model catalog before testing.')}</option>{availableSubscriptionTargets.map((entry) => <option key={entry.internal_id} value={entry.model_id}>{entry.name?.trim() || entry.model_id}{entry.eligibility === 'eligible' ? '' : ` · ${t('Eligibility unknown or unavailable')}`}</option>)}{provider.test_model && !providerTypeChanged && !availableSubscriptionTargets.some((entry) => entry.model_id === provider.test_model) && <option value={provider.test_model}>{provider.test_model} · {t('Saved test target')}</option>}</select><p className="provider-test-help" data-testid={providerTypeChanged ? 'provider-kind-change-warning' : undefined}>{t(providerTypeChanged ? 'Save this provider type and refresh its model catalog before testing.' : 'A subscription provider test sends a short Chat text request through the shared admission gate. It may consume subscription allowance.')}</p></div>}
          {!subscription && <div className="form-field"><label htmlFor="provider-test-model">{t('Test model')}</label><div className="provider-test-fields"><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} id="provider-test-model" value={provider.test_model} placeholder={t('Enter a model ID')} onChange={(e) => setProvider({ ...provider, test_model: e.target.value })} /><Select searchable={false} aria-label={t('Test type')} value="text" onChange={() => {}}><option value="text">{t('Text generation')}</option></Select></div><p className="provider-test-help">{t('Send a minimal text request using the selected API format to verify the URL and credential. Upstream usage charges may apply.')}</p><p className="provider-test-endpoint">{t('Test endpoint')}: <code>{endpoint}</code></p></div>}
        </fieldset>
        {source && !edit && <label className="model-selection api-source-add-model"><input data-testid="api-source-add-model" type="checkbox" checked={addSourceModel} onChange={event => setAddSourceModel(event.target.checked)}/>{t('Add the entered model to my pool when saving')}</label>}
        {source && !edit && <p className="provider-test-help">{t('Save this source connection and model before testing. Saving sends no request.')}</p>}
        {testResult && <p className={cx('provider-test-result', testFailed && 'error')} role="status">{testResult}</p>}
        <div className="provider-dialog-actions">{(!subscription || edit) && <button type="button" className="button ghost" data-testid={subscription ? 'provider-subscription-test-action' : undefined} disabled={testing || (Boolean(source) && !edit) || (subscription && (providerTypeChanged || !provider.test_model.trim()))} onClick={() => void test()}>{testing ? <LoaderCircle size={16} className="import-spinner" /> : <Play size={16} />}{t(testing ? 'Testing…' : 'Test')}</button>}<button className="button primary" type="submit" disabled={testing}>{t('Save provider')}</button></div>
      </form>
    </div>
  </div>;
}

function ModelDialog({ initial, providers, onClose, onSave, onTestStatus }: { onTestStatus: (id: string, status: ProviderTestStatus) => void | Promise<void>; initial: Model; providers: Provider[]; onClose: () => void; onSave: (m: Model) => Promise<void> }) {
  const { t } = usePreferences();
  const enabledProviders = providers.filter((provider) => provider.enabled);
  const initialProvider = enabledProviders.find((p) => p.id === initial.provider_id);
  const [model, setModel] = useState({ ...initial, provider_id: initialProvider?.id ?? '', api_type: normalizeApiType(initial.api_type || initialProvider?.api_type), cache_cost_per_million: initial.cache_cost_per_million ?? 0 });
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [result, setResult] = useState('');
  const [failed, setFailed] = useState(false);
  const form = useRef<HTMLFormElement>(null);
  const edit = Boolean(initial.id);
  const provider = enabledProviders.find((p) => p.id === model.provider_id);
  const subscription = provider ? isSubscriptionProvider(provider) : false;
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (testing || saving || !provider) return;
    setSaving(true); setResult('');
    try { await onSave({ ...model, id: model.id || crypto.randomUUID(), model_id: model.model_id.trim(), name: model.name.trim() || model.model_id.trim() }); }
    catch (error) { setFailed(true); setResult(providerTestError(error, t)); }
    finally { setSaving(false); }
  };
  const test = async () => {
    if (testing || !form.current?.reportValidity() || !provider) return;
    setTesting(true); setResult(''); setFailed(false); await onTestStatus(provider.id, 'testing');
    try {
      const testResult = await testProviderDraft({ ...provider, api_type: isSubscriptionProvider(provider) ? '' : model.api_type, test_model: model.model_id.trim() });
      setResult(t(testResult)); await onTestStatus(provider.id, 'success');
    } catch (error) { setFailed(true); setResult(providerTestError(error, t)); await onTestStatus(provider.id, 'error'); }
    finally { setTesting(false); }
  };
  return <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !testing && !saving) onClose(); }}>
    <div className="dialog provider-dialog" role="dialog" aria-modal="true" aria-labelledby="model-dialog-title">
      <div className="provider-dialog-header"><div><h2 id="model-dialog-title">{t(edit ? 'Configure model' : 'Add model')}</h2><p>{t(subscription ? 'Configure a subscription model and test it through the shared generation gate.' : 'Configure a model and test it with the provider’s saved API key.')}</p></div><button type="button" className="icon-action" data-testid="model-dialog-close" disabled={testing || saving} onClick={onClose} aria-label={t('Close')}><X size={20} /></button></div>
      <form autoComplete="off" ref={form} className="provider-dialog-form" onSubmit={submit} onChange={() => setResult('')}>
        {!enabledProviders.length && <p className="provider-test-help">{t('Enable a provider before adding a model.')}</p>}
        <fieldset disabled={testing || saving}>
          <div className="field-pair">
            <div className="form-field"><span>{t('Provider')}</span><Select required aria-label={t('Provider')} renderIcon={(id) => { const selected = enabledProviders.find((p) => p.id === id); return selected ? <ProviderLogo id={providerPreset(selected)} /> : null; }} value={provider?.id ?? ''} onChange={(event) => {
              const next = enabledProviders.find((p) => p.id === event.target.value);
              setModel({ ...model, provider_id: event.target.value, api_type: normalizeApiType(next?.api_type) }); setResult('');
            }}><option value="">{t('Select provider')}</option>{enabledProviders.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}</Select></div>
            <div className="form-field"><span>{t('API type')}</span><Select searchable={false} aria-label={t('API type')} value={model.api_type} onChange={(event) => { setModel({ ...model, api_type: event.target.value }); setResult(''); }}><option value="chat_completions">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="messages">Anthropic Messages</option></Select></div>
          </div>
          {subscription && <p className="provider-test-help" data-testid="subscription-test-protocol-note">{t('The subscription test is locked to Chat Completions text. The API type above configures normal routing and is not verified by this test.')}</p>}
          <div className="field-pair">
            <label className="form-field"><span>{t('Provider model ID')}</span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} required value={model.model_id} placeholder={t('Enter a model ID')} onChange={(event) => setModel({ ...model, model_id: event.target.value })} /></label>
            <label className="form-field"><span>{t('Display name')} <small>{t('(optional)')}</small></span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} value={model.name} placeholder={model.model_id || t('Display name')} onChange={(event) => setModel({ ...model, name: event.target.value })} /></label>
          </div>
          <div className="model-cost-fields">
            <label className="form-field"><span>{t('Input cost')}</span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} type="number" min="0" step="any" placeholder={t('Unknown')} value={(model.input_price_known ?? (model.input_cost_per_million ?? 0) > 0) ? model.input_cost_per_million : ''} onChange={(event) => setModel({ ...model, input_price_known: event.target.value !== '', input_cost_per_million: Number(event.target.value) })} /></label>
            <label className="form-field"><span>{t('Output cost')}</span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} type="number" min="0" step="any" placeholder={t('Unknown')} value={(model.output_price_known ?? (model.output_cost_per_million ?? 0) > 0) ? model.output_cost_per_million : ''} onChange={(event) => setModel({ ...model, output_price_known: event.target.value !== '', output_cost_per_million: Number(event.target.value) })} /></label>
            <label className="form-field"><span>{t('Cache cost')}</span><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} type="number" min="0" step="any" placeholder={t('Unknown')} value={(model.cache_price_known ?? (model.cache_cost_per_million ?? 0) > 0) ? model.cache_cost_per_million : ''} onChange={(event) => setModel({ ...model, cache_price_known: event.target.value !== '', cache_cost_per_million: Number(event.target.value) })} /></label>
          </div>
          <p className="provider-test-help">{t('Prices in USD per 1M tokens. Cache cost is the cached input price.')} {t('Leave pricing blank if unknown; enter 0 only for a confirmed free rate.')}</p>
          <div className="field-pair model-capability-fields">
            <div className="form-field"><label className="model-image-label" htmlFor="model-image-input">{t('Image input')}</label><div className="model-image-control"><input id="model-image-input" autoComplete="off" autoCapitalize="none" type="checkbox" checked={model.supports_vision} onChange={event => setModel({ ...model, supports_vision: event.target.checked })}/></div></div>
            <label className="form-field"><span>{t('Context length (tokens)')}</span><input autoComplete="off" autoCapitalize="none" type="number" min="1" max="9007199254740991" step="1" placeholder={t('Unknown')} value={model.context_window || ''} onChange={event => setModel({ ...model, context_window: event.target.value === '' ? 0 : Number(event.target.value) })}/></label>
          </div>
          <p className="provider-test-help">{t('Image requests only use models marked as supporting image input. Context length is informational only.')}</p>
          <p className="provider-test-help">{t(subscription ? 'Subscription tests use a discovered model through the shared generation gate. Subscription usage may apply.' : 'Testing sends a minimal text request using the saved API key. Upstream usage charges may apply.')}</p>
        </fieldset>
        {result && <p className={cx('provider-test-result', failed && 'error')} role="status">{result}</p>}
        <div className="provider-dialog-actions"><button type="button" className="button ghost" disabled={testing || saving || !provider} onClick={() => void test()}>{testing ? <LoaderCircle size={16} className="import-spinner" /> : <Play size={16} />}{t(testing ? 'Testing…' : 'Test')}</button><button type="submit" className="button primary" disabled={testing || saving || !provider}><Save size={15} />{t('Save model')}</button></div>
      </form>
    </div>
  </div>;
}

function Toggle({ checked, onChange, title, body }: { checked: boolean; onChange: (v: boolean) => void; title: string; body: string }) {
  return <label className="toggle-row"><button type="button" role="switch" aria-checked={checked} className={cx('switch', checked && 'on')} onClick={() => onChange(!checked)}><span /></button><span><strong>{title}</strong><small>{body}</small></span></label>;
}

function PanelHeader({ eyebrow, title, action }: { eyebrow: string; title: string; action?: React.ReactNode }) {
  return <div className="panel-head"><div><span>{eyebrow}</span><h3>{title}</h3></div>{action}</div>;
}

function PageIntro({ title, body, action }: { title: string; body: string; action?: React.ReactNode }) {
  return <section className="page-intro"><div><h2>{title}</h2><p>{body}</p></div>{action}</section>;
}

function FieldLabel({ label, help }: { label: string; help?: string }) {
  return <label className="field-label"><span>{label}</span>{help && <small>{help}</small>}</label>;
}

function EmptyState({ icon, title, body }: { icon: React.ReactNode; title: string; body: string }) {
  return <div className="empty-state"><span>{icon}</span><strong>{title}</strong><p>{body}</p></div>;
}
