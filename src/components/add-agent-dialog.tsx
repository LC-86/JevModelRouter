import { useRef, useState } from 'react';
import { Bot, ImagePlus, LoaderCircle, Play, X } from 'lucide-react';
import { usePreferences } from '../lib/preferences-context';
import { saveCustomAgent, testCustomAgent } from '../lib/bridge';
import type { DashboardSnapshot, AgentStatus } from '../types';

export function AddAgentDialog({ initial, onClose, onSaved }: { initial?: AgentStatus; onClose: () => void; onSaved: (snapshot: DashboardSnapshot) => void }) {
  const { t } = usePreferences();
  const [id] = useState(() => initial?.id ?? 'custom-' + crypto.randomUUID());
  const [name, setName] = useState(initial?.name ?? '');
  const [command, setCommand] = useState(initial?.command ?? '');
  const [args, setArgs] = useState(initial?.args?.join('\n') ?? '');
  const [icon, setIcon] = useState<string | null>(initial?.icon ?? null);
  const [configPath, setConfigPath] = useState(initial?.injection?.path ?? initial?.config_path ?? '');
  const [busy, setBusy] = useState<'save' | 'test' | null>(null);
  const [error, setError] = useState('');
  const [result, setResult] = useState('');
  const picker = useRef<HTMLInputElement>(null);
  const valid = !!name.trim() && !!command.trim();
  const agent = () => ({ id, name: name.trim(), command: command.trim(), args: args.split(/\r?\n/).filter(line => line.trim().length > 0), icon, config_path: configPath.trim() });
  const changed = () => { setError(''); setResult(''); };
  async function selectImage(file?: File) {
    if (!file) return;
    if (!['image/png', 'image/jpeg', 'image/webp'].includes(file.type) || file.size > 2 * 1024 * 1024) { setError(t('Choose a PNG, JPEG or WebP image under 2 MB.')); return; }
    try {
      const bitmap = await createImageBitmap(file);
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 128;
      const size = Math.min(bitmap.width, bitmap.height);
      canvas.getContext('2d')!.drawImage(bitmap, (bitmap.width - size) / 2, (bitmap.height - size) / 2, size, size, 0, 0, 128, 128);
      bitmap.close(); setIcon(canvas.toDataURL('image/png')); setError('');
    } catch { setError(t('Could not read the image.')); }
  }
  async function test() {
    setBusy('test'); changed();
    try { setResult(await testCustomAgent(agent())); } catch (e) { setError(t(String(e))); } finally { setBusy(null); }
  }
  return <div className="modal-backdrop" onKeyDown={e => { if (e.key === 'Escape' && !busy) onClose(); }}><div className="dialog provider-dialog add-agent-dialog" role="dialog" aria-modal="true" aria-labelledby="add-agent-title">
    <div className="provider-dialog-header"><h2 id="add-agent-title">{t(initial ? 'Edit local agent' : 'Add local agent')}</h2><button type="button" className="icon-action" disabled={!!busy} aria-label={t('Close')} onClick={onClose}><X size={20}/></button></div>
    <form className="provider-dialog-form" onSubmit={async e => {
      e.preventDefault(); if (busy || !valid) return; setBusy('save'); setError('');
      try { onSaved(await saveCustomAgent(agent())); onClose(); } catch (e) { setError(t(String(e))); } finally { setBusy(null); }
    }}>
      <fieldset disabled={!!busy} className="add-agent-fields">
        <div className="add-agent-avatar-row"><button type="button" className="add-agent-avatar" aria-label={t('Choose image')} onClick={() => picker.current?.click()}>{icon ? <img src={icon} alt=""/> : <Bot size={30}/>}<span><ImagePlus size={14}/></span></button><button type="button" className="button ghost" onClick={() => picker.current?.click()}>{t('Choose image')}</button>{icon && <button type="button" className="icon-action" aria-label={t('Remove image')} onClick={() => setIcon(null)}><X size={16}/></button>}<input ref={picker} hidden type="file" accept="image/png,image/jpeg,image/webp" onChange={e => { void selectImage(e.target.files?.[0]); e.target.value = ''; }}/></div>
        <label className="form-field">{t('Agent name')}<input autoFocus required value={name} onChange={e => { setName(e.target.value); changed(); }}/></label>
        <label className="form-field">{t('Executable path')}<input required placeholder="/path/to/agent" value={command} spellCheck={false} autoComplete="off" onChange={e => { setCommand(e.target.value); changed(); }}/></label>
        <label className="form-field">{t('Configuration file path (optional)')}<input value={configPath} placeholder={t('Leave blank to detect the default path')} onChange={e => { setConfigPath(e.target.value); changed(); }}/></label>
        <p className="add-agent-help">{t('AutoJev detects how to connect from the executable and configuration path. Unknown agents remain manually configured.')}</p>
        <label className="form-field">{t('Launch arguments')}<textarea rows={3} placeholder={'--model\nmy-model'} value={args} spellCheck={false} onChange={e => { setArgs(e.target.value); changed(); }}/></label>
        <p className="add-agent-help">{t('One argument per line. Use {prompt} for the message; otherwise it is appended as the last argument.')}</p>
        <p className="add-agent-help">{t('Testing runs the executable with its current configuration and may consume quota. Save first, then connect from the agent list to apply gateway settings.')}</p>
      </fieldset>
      {error && <div className="add-agent-result route-error" role="alert">{error}</div>}
      {result && <div className="add-agent-result" role="status"><strong>{t('Test passed')}</strong><pre>{result}</pre></div>}
      <div className="provider-dialog-actions add-agent-actions"><button type="button" className="button ghost add-agent-test" disabled={!!busy || !valid} onClick={() => void test()}>{busy === 'test' ? <LoaderCircle size={16} className="import-spinner"/> : <Play size={16}/>} {t(busy === 'test' ? 'Testing…' : 'Test connection')}</button><button type="button" className="button ghost" disabled={!!busy} onClick={onClose}>{t('Cancel')}</button><button className="button primary" disabled={!!busy || !valid}>{busy === 'save' && <LoaderCircle size={16} className="import-spinner"/>}{t(busy === 'save' ? 'Saving…' : 'Save')}</button></div>
    </form>
  </div></div>;
}
