import { useEffect, useRef, useState } from 'react';
import { Check, Copy, LoaderCircle, LogIn, LogOut, RefreshCw, ShieldCheck, X } from 'lucide-react';
import {
  beginSubscriptionLogin, cancelSubscriptionLogin, logoutSubscription, pollSubscriptionLogin, switchSubscriptionAccount,
} from '../lib/bridge';
import { authErrorLabel, authPhaseLabel, cancelAllowed, logoutLocalLabel, pollResultAllowed, pollTickAllowed, remoteRevokeLabel, subscriptionAuthView } from '../lib/subscription';
import { usePreferences } from '../lib/preferences-context';
import type { DashboardSnapshot, Provider } from '../types';

const POLL_MS = 1500;

/** 订阅登录/退出/换号对话框：只对订阅服务商开放，界面不持有也不显示任何凭据。 */
export function SubscriptionAuthDialog({ provider, snapshot, onSnapshot, onNotify, onClose }: {
  provider: Provider;
  snapshot: DashboardSnapshot;
  onSnapshot: (snapshot: DashboardSnapshot) => void;
  onNotify?: (message: string, error?: boolean) => void;
  onClose: () => void;
}) {
  const { t } = usePreferences();
  const dialog = useRef<HTMLDialogElement>(null);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState('');
  const [copied, setCopied] = useState('');
  const [polling, setPolling] = useState(false);
  // poll 在途用 ref 同步记录（定时器闭包读不到最新 state）；busy 亦然。
  const pollInFlight = useRef(false);
  const busyRef = useRef(false);
  // 任何新命令都会推进序号：cancel/logout 之后回来的迟到 poll 响应据此丢弃。
  const pollEpoch = useRef(0);
  const view = subscriptionAuthView(snapshot, provider.id);
  const helper = view?.helper;
  const errorText = (cause: unknown) => String(cause instanceof Error ? cause.message : cause);

  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    return () => { element.close(); previous?.focus(); };
  }, []);

  // 只在 pending 时轮询；相位离开 pending 或卸载立即清除定时器。
  // tick 同时尊重「停止／其它命令在途／上次 poll 未返回」，取消后迟到的响应按序号丢弃。
  useEffect(() => {
    if (view?.phase !== 'pending') return;
    let active = true;
    let timer = 0;
    const tick = async () => {
      if (!pollTickAllowed({ phase: view?.phase ?? 'idle', busy: busyRef.current, inFlight: pollInFlight.current, stopped: !active })) return;
      const requestEpoch = pollEpoch.current;
      pollInFlight.current = true;
      setPolling(true);
      try {
        const next = await pollSubscriptionLogin(provider.id);
        if (pollResultAllowed({ phase: view?.phase ?? 'idle', requestEpoch, currentEpoch: pollEpoch.current, stopped: !active })) onSnapshot(next);
      } catch (cause) {
        if (active) { setFailure(errorText(cause)); window.clearInterval(timer); }
      } finally {
        if (active) { pollInFlight.current = false; setPolling(false); }
      }
    };
    timer = window.setInterval(() => void tick(), POLL_MS);
    return () => { active = false; window.clearInterval(timer); pollInFlight.current = false; };
  }, [view?.phase, provider.id, onSnapshot]);

  // 所有命令都返回完整快照，界面直接替换，不自行推断状态。
  const run = async (action: () => Promise<DashboardSnapshot>, done?: string) => {
    if (busyRef.current) return;
    pollEpoch.current += 1;
    busyRef.current = true;
    setBusy(true);
    setFailure('');
    try { onSnapshot(await action()); if (done) onNotify?.(done); }
    catch (cause) { setFailure(errorText(cause)); }
    finally { busyRef.current = false; setBusy(false); }
  };

  const copy = async (label: string, value: string) => {
    try { await navigator.clipboard.writeText(value); setCopied(label); window.setTimeout(() => setCopied(''), 1500); }
    catch { setFailure(t('Copying to the clipboard is unavailable in this window.')); }
  };

  const challenge = view?.challenge;
  const verificationUrl = challenge?.verification_url ?? '';
  const userCode = challenge?.user_code ?? '';
  const localDetail = view?.logout.local_detail ?? '';
  const remoteDetail = view?.logout.remote_detail ?? '';
  return (
    <dialog ref={dialog} className="dialog subscription-auth-dialog" aria-labelledby="subscription-auth-title" onCancel={(event) => { event.preventDefault(); if (!busy) onClose(); }}>
      <div className="subscription-auth-header">
        <div><h2 id="subscription-auth-title">{t('Subscription sign-in')}</h2><p>{provider.name} · <code>{provider.id}</code></p>{view && <p className="subscription-auth-generation">{t('Generation {generation}', { generation: view.generation })}{view.attempt != null && ` · ${t('Attempt {attempt}', { attempt: view.attempt })}`}</p>}</div>
        <button type="button" className="icon-action" aria-label={t('Close')} disabled={busy} onClick={onClose}><X size={20} /></button>
      </div>

      <section className="subscription-auth-section">
        <h3><ShieldCheck size={15} aria-hidden="true" />{t('Official helper')}</h3>
        <dl className="subscription-auth-facts">
          <div><dt>{t('Helper availability')}</dt><dd className={helper?.available ? 'subscription-auth-ok' : 'subscription-auth-missing'}>{t(helper?.available ? 'Helper available' : 'Helper unavailable')}</dd></div>
          <div><dt>{t('Helper program')}</dt><dd><code>{helper?.program ?? t('Unknown')}</code></dd></div>
          <div><dt>{t('Helper version')}</dt><dd>{helper?.version ?? t('Unknown')}</dd></div>
          <div><dt>{t('Authorization storage')}</dt><dd><code>{helper?.home ?? t('Unknown')}</code></dd></div>
        </dl>
        <p className="subscription-auth-note">{t('The helper is only started for sign-in; availability is probed without network access.')}</p>
      </section>

      <section className="subscription-auth-section">
        <h3>{t('Sign-in status')}</h3>
        <p className="subscription-auth-phase" role="status">{authPhaseLabel(view?.phase ?? 'idle', t)}</p>
        {view?.phase === 'pending' && challenge && <div className="subscription-auth-challenge">
          <p>{challenge.instructions}</p>
          <p className="subscription-auth-challenge-method">{t('Sign-in method')}: <code>{challenge.kind}</code></p>
          {verificationUrl && <div className="subscription-auth-copy"><code>{verificationUrl}</code><button type="button" className="button small ghost" onClick={() => void copy('url', verificationUrl)}>{copied === 'url' ? <Check size={14} /> : <Copy size={14} />}{t(copied === 'url' ? 'Copied' : 'Copy')}</button></div>}
          {userCode && <div className="subscription-auth-copy"><code>{userCode}</code><button type="button" className="button small ghost" onClick={() => void copy('code', userCode)}>{copied === 'code' ? <Check size={14} /> : <Copy size={14} />}{t(copied === 'code' ? 'Copied' : 'Copy')}</button></div>}
        </div>}
        {view?.phase === 'succeeded' && <p className="subscription-auth-identity" role="status"><ShieldCheck size={16} aria-hidden="true" /><span>{t('Identity verified')}</span><code>{view.identity ?? t('Unknown')}</code></p>}
        {(failure || view?.phase === 'failed') && <div className="subscription-auth-error" role="alert">
          <strong>{t('Sign-in failed')}</strong>
          {view?.error && <p className="subscription-auth-error-label">{authErrorLabel(view.error, t)}</p>}
          {view?.error?.message && view.error.message !== authErrorLabel(view.error, t) && <p className="subscription-auth-error-detail">{view.error.message}</p>}
          {view?.error?.recovery && <p className="subscription-auth-recovery">{t('Recovery')}: {view.error.recovery}</p>}
          {failure && <p className="subscription-auth-error-detail">{failure}</p>}
        </div>}
        <div className="subscription-auth-actions">
          {view?.phase === 'pending'
            ? <button type="button" className="button ghost" disabled={!cancelAllowed({ phase: view?.phase ?? 'idle', busy, pollInFlight: polling })} title={polling ? t('Waiting for the current poll to finish') : undefined} onClick={() => void run(() => cancelSubscriptionLogin(provider.id))}>{busy ? <LoaderCircle size={16} className="import-spinner" /> : <X size={16} />}{t('Cancel sign-in')}</button>
            : <button type="button" className="button primary subscription-auth-begin" disabled={busy} onClick={() => void run(() => beginSubscriptionLogin(provider.id))}>{busy ? <LoaderCircle size={16} className="import-spinner" /> : <LogIn size={16} />}{t('Sign in')}</button>}
        </div>
      </section>

      <section className="subscription-auth-section">
        <h3><LogOut size={15} aria-hidden="true" />{t('Sign-out evidence')}</h3>
        <dl className="subscription-auth-facts">
          <div><dt>{t('Local clearing')}</dt><dd>{logoutLocalLabel(view?.logout.local ?? 'not_attempted', t)}{localDetail && <small>{localDetail}</small>}</dd></div>
          <div><dt>{t('Remote revoke')}</dt><dd>{remoteRevokeLabel(view?.logout.remote ?? 'not_attempted', t)}{remoteDetail && <small>{remoteDetail}</small>}</dd></div>
        </dl>
        <p className="subscription-auth-note">{t('Local clearing and remote revoke are tracked separately; this app never reports a remote revoke it did not perform.')}</p>
      </section>

      <div className="subscription-auth-footer">
        <button type="button" className="button ghost" disabled={busy} onClick={() => void run(() => logoutSubscription(provider.id), t('Subscription signed out'))}><LogOut size={16} />{t('Sign out')}</button>
        <button type="button" className="button ghost" disabled={busy} onClick={() => void run(() => switchSubscriptionAccount(provider.id), t('Account switch started'))}><RefreshCw size={16} />{t('Switch account')}</button>
        <button type="button" className="button primary" disabled={busy} onClick={onClose}>{t('Close')}</button>
      </div>
    </dialog>
  );
}
