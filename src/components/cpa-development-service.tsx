import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type ServiceStatus = {
  state: 'stopped' | 'ready' | 'exited' | 'missing';
  binary: string;
  port: number;
  service: { running: boolean; pid: number | null; base_url: string } | null;
  real_generation_enabled: false;
  artifact: { version: string; platform: string };
};
const labels = { stopped: '已停止', ready: '隔离服务就绪', exited: '服务已退出', missing: '固定服务文件缺失' };

export function CpaDevelopmentService() {
  const [status, setStatus] = useState<ServiceStatus | null>(null);
  const [binary, setBinary] = useState('');
  const [port, setPort] = useState('0');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let timer: ReturnType<typeof setInterval> | undefined;
    const read = () => invoke<ServiceStatus | null>('get_cpa_development_service');
    void read().then(value => {
      if (disposed || !value) return;
      setStatus(value); setBinary(value.binary); setPort(String(value.port));
      // Observe only the owned child handle. No auth, quota or model requests.
      timer = setInterval(() => void read().then(next => { if (!disposed) setStatus(next); })
        .catch(e => { if (!disposed) setError(String(e)); }), 1000);
    }).catch(() => { /* Ordinary builds do not contain this development entry. */ });
    return () => { disposed = true; if (timer) clearInterval(timer); };
  }, []);
  if (!status) return null;
  const run = async (stop: boolean) => {
    setBusy(true); setError('');
    try {
      if (stop) await invoke('stop_cpa_validation');
      else {
        const value = Number(port);
        if (!Number.isInteger(value) || value < 0 || value > 65535) throw new Error('使用 0 自动分配或有效的独立端口');
        const service = await invoke<{ base_url: string }>('start_cpa_development_service', { binary, port: value });
        setPort(new URL(service.base_url).port);
      }
      setStatus(await invoke<ServiceStatus>('get_cpa_development_service'));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };
  return <section className="table-panel cpa-subscriptions" data-testid="cpa-development-service" aria-label="隔离 CPA 开发服务">
    <div className="cpa-subscriptions-intro">
      <strong>隔离 CPA 开发服务</strong>
      <p role="status" data-testid="cpa-service-state">{labels[status.state]} · {status.artifact.version} · {status.artifact.platform}</p>
      <p>真实能力禁用；此服务只接虚构回环来源。启动或恢复不改变身份、模型、协议与费用准入。</p>
      {status.service && <p>本地地址：{status.service.base_url} · 自有进程：{status.service.pid ?? '已退出'}</p>}
      <label>固定 CPA 文件 <input data-testid="cpa-service-binary" value={binary} disabled={busy || status.state === 'ready'} onChange={e => setBinary(e.target.value)} /></label>
      <label>独立端口（0 自动分配） <input data-testid="cpa-service-port" inputMode="numeric" value={port} disabled={busy || status.state === 'ready'} onChange={e => setPort(e.target.value)} /></label>
      {status.port > 0 && <p>恢复须使用已保存端口 {status.port}。端口被占用时先由其所有者释放；改端口须用新隔离目录并建立新的来源与模型引用。</p>}
      {error && <p role="alert" data-testid="cpa-service-error">{error}。核对固定文件/版本；首次启动可选独立端口，恢复须保留绑定端口。不回收外部实例。</p>}
      {status.state === 'exited' && <p>自有服务已退出。核对文件和端口后点恢复；保留来源与模型引用，生成仍受原准入限制。</p>}
      <div className="provider-actions">
        <button className="button" data-testid="cpa-service-start" disabled={busy || status.state === 'ready'} onClick={() => void run(false)}>{status.state === 'exited' ? '恢复自有服务' : '启动自有服务'}</button>
        <button className="button" data-testid="cpa-service-stop" disabled={busy || !status.service} onClick={() => void run(true)}>停止自有服务</button>
      </div>
    </div>
  </section>;
}
