import { DebugCurl } from './debug-curl';
import { makeCurl } from '../lib/debug-curl';
import { DebugDetailButton } from './debug-detail-dialog';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Send, Plus, Trash2, X, LoaderCircle, FileText } from 'lucide-react';
import type { DashboardSnapshot, RoutePreviewInput } from '../types';
import { debugRequest, type DebugResult } from '../lib/bridge';
import { DebugMarkdown } from './debug-markdown';
import { debugResponseWarning, debugContent, debugHistory, parseDebugParameters, debugRequestBody, debugResponseText, type DebugMessage, type DebugFile } from '../lib/debug-message';
import { DebugMessageDetails } from './debug-message-details';
import { providerIdentifier } from '../lib/provider-name';
import { usePreferences } from '../lib/preferences-context';
import { SearchSelect } from './search-select';
import { Select } from './select';
import { DEBUG_PREFERENCES_KEY, parseDebugPreferences } from '../lib/debug-preferences';
export function DebugPage({ snapshot }: { snapshot: DashboardSnapshot }) {
  const { t } = usePreferences();
  const models = snapshot.models.filter(m => m.enabled && snapshot.providers.some(p => p.id === m.provider_id && p.enabled));
  const options = [
    ...snapshot.routes.filter(r => r.enabled && models.some(m => (r.strategy === 'jev' && r.all_models) || r.model_ids.includes(m.id))).map(r => ({ value: 'autojev/' + r.id, label: r.id, group: t('Routes') })).sort((a,b) => a.label.localeCompare(b.label)),
    ...models.map(m => ({ value: 'autojev/model/' + m.id, label: providerIdentifier(snapshot.providers.find(p => p.id === m.provider_id)!) + '/' + m.model_id, group: t('Models') })).sort((a,b) => a.label.localeCompare(b.label))
  ];
  const [saved] = useState(() => {
    let raw: string | null = null;
    try { raw = localStorage.getItem(DEBUG_PREFERENCES_KEY); } catch { /* Storage unavailable. */ }
    return parseDebugPreferences(raw, options.map(option => option.value));
  });
  const [target, setTarget] = useState(saved.target);
  const [endpoint, setEndpoint] = useState<RoutePreviewInput['endpoint']>(saved.endpoint);
  const [mode, setMode] = useState<'chat' | 'curl'>('chat');
  const [curlInitial, setCurlInitial] = useState('');
  const [prompt, setPrompt] = useState('');
  const [files, setFiles] = useState<DebugFile[]>([]);
  const fileInput = useRef<HTMLInputElement>(null);
  const composerInput = useRef<HTMLTextAreaElement>(null);
  const reading = useRef(false);
  useLayoutEffect(() => {
    const input = composerInput.current;
    if (!input) return;
    const resize = () => { input.style.height = '0px'; input.style.height = `${Math.min(160, input.scrollHeight + 2)}px`; };
    resize();
    let width = input.clientWidth;
    const widthObserver = new ResizeObserver(() => { if (input.clientWidth !== width) { width = input.clientWidth; resize(); } });
    widthObserver.observe(input);
    return () => widthObserver.disconnect();
  }, [prompt, mode]);
  const [images, setImages] = useState<string[]>([]);
  const [readingImages, setReadingImages] = useState(false);
  const [messages, setMessages] = useState<DebugMessage[]>([]);
  const [parameters, setParameters] = useState(saved.parameters);
  const [session, setSession] = useState(() => crypto.randomUUID());
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    try { localStorage.setItem(DEBUG_PREFERENCES_KEY, JSON.stringify({ target, endpoint, parameters })); }
    catch { /* Storage failure must not interrupt debugging. */ }
  }, [target, endpoint, parameters]);
  const sending = useRef(false);
  const messageList = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const list = messageList.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, [messages, busy, error]);
  const clear = () => { setMessages([]); setError(''); setSession(crypto.randomUUID()); };
  const valid = options.some(o => o.value === target) && (!!prompt.trim() || images.length > 0 || files.length > 0) && !readingImages;
  const run = async () => {
    if (!valid || sending.current || reading.current) return;
    let params: Record<string, unknown>;
    try { params = parseDebugParameters(parameters); }
    catch { setError(t('Invalid or duplicate parameter')); return; }
    sending.current = true;
    setError(''); setBusy(true);
    const current = prompt.trim();
    const userId = crypto.randomUUID(), assistantId = crypto.randomUUID();
    const history = debugHistory(messages, endpoint);
    const contentInput = debugContent(current, images, endpoint, files);
    const request = { target, endpoint, parameters: { ...params } };
    setMessages(previous => [...previous,
      { id: userId, role: 'user', status: 'pending', content: current, images: [...images], files: [...files], raw: JSON.stringify(debugRequestBody(target, endpoint, contentInput, history, params), null, 2) },
      { id: assistantId, role: 'assistant', status: 'pending', content: '', request },
    ]);
    setPrompt(''); setImages([]); setFiles([]);
    let reply: DebugResult | undefined;
    try {
      reply = await debugRequest(target, endpoint, contentInput, history, params, session, event => {
        setMessages(previous => previous.map(message => message.id === assistantId && message.status === 'pending' ? { ...message, content: message.content + event.text, reasoning: event.reasoning && !event.text } : message));
      });
      let data: unknown;
      try { data = reply.parsed_body ?? JSON.parse(reply.body); } catch { throw new Error('Upstream returned invalid JSON'); }
      if (reply.status >= 400) {
        const detail = (data as { error?: { message?: string } })?.error?.message;
        throw new Error(typeof detail === 'string' ? detail : `HTTP ${reply.status}`);
      }
      const content = debugResponseText(data);
      const warning = debugResponseWarning(data);
      if (!content && !warning) throw new Error('No text response; inspect JSON below');
      setMessages(previous => previous.map(message => message.id === userId ? { ...message, status: content ? 'success' : 'error', raw: reply?.request_body ? JSON.stringify(reply.request_body, null, 2) : message.raw } :
        message.id === assistantId ? { ...message, status: content ? 'success' : 'error', content, warning, raw: reply?.parsed_body ? reply.body : JSON.stringify(data, null, 2), result: reply } : message));
    } catch (e) {
      setMessages(previous => previous.map(message => message.id === userId ? { ...message, status: 'error', raw: reply?.request_body ? JSON.stringify(reply.request_body, null, 2) : message.raw } :
        message.id === assistantId ? { ...message, status: 'error', content: message.content, warning: t(e instanceof Error ? e.message : String(e)), raw: reply?.body, result: reply } : message));
    } finally { sending.current = false; setBusy(false); }
  };
  const addAttachments = async (selected: File[]) => {
    if (!selected.length || busy || reading.current) return;
    if (images.length + files.length + selected.length > 4 || selected.some(file => file.size > 4 * 1024 * 1024)) {
      setError(t('Add up to 4 attachments, each under 4 MB.')); return;
    }
    reading.current = true; setReadingImages(true); setError('');
    try {
      const added = await Promise.all(selected.map(async file => {
        const image = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(file.type);
        const pdf = file.type === 'application/pdf' || /\.pdf$/i.test(file.name);
        if (image || pdf) {
          const data = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(new Error('Could not read attachment'));
            reader.readAsDataURL(pdf ? new Blob([file], { type: 'application/pdf' }) : file);
          });
          return { image, file: { name: file.name, size: file.size, data } };
        }
        let text: string;
        try { text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer()); }
        catch { throw new Error('Use images, PDF, or UTF-8 text and code files.'); }
        if (text.includes('\0')) throw new Error('Use images, PDF, or UTF-8 text and code files.');
        return { image: false, file: { name: file.name, size: file.size, text } };
      }));
      setImages(previous => [...previous, ...added.filter(item => item.image).map(item => item.file.data!)]);
      setFiles(previous => [...previous, ...added.filter(item => !item.image).map(item => item.file)]);
    } catch (e) { setError(t(e instanceof Error ? e.message : 'Could not read attachment')); }
    finally { reading.current = false; setReadingImages(false); }
  };
  return <div className="debug-console">
    <div className="page-intro"><div><h2>{t('Debug')}</h2><p>{t('Send real requests through AutoJev to test models and routes.')}</p></div></div>
    <div className="debug-mode-tabs" role="tablist" aria-label={t('Debug mode')}>
      <button role="tab" id="debug-chat-tab" aria-controls="debug-chat-panel" aria-selected={mode === 'chat'} disabled={busy} onClick={() => setMode('chat')}>{t('Conversation mode')}</button>
      <button role="tab" id="debug-curl-tab" aria-controls="debug-curl-panel" aria-selected={mode === 'curl'} disabled={busy} onClick={() => {
        if (mode === 'curl') return;
        try {
          setCurlInitial(makeCurl(snapshot.proxy.port, endpoint, debugRequestBody(target, endpoint, debugContent(prompt.trim() || 'Hello', images, endpoint, files), debugHistory(messages, endpoint), parseDebugParameters(parameters))));
          setMode('curl'); setError('');
        } catch { setError(t('Invalid or duplicate parameter')); }
      }}>{t('cURL mode')}</button>
    </div>
    {mode === 'curl' && <div className="debug-mode-panel" id="debug-curl-panel" role="tabpanel" aria-labelledby="debug-curl-tab"><DebugCurl initial={curlInitial} port={snapshot.proxy.port}/></div>}
    <div className="debug-layout" id="debug-chat-panel" role="tabpanel" aria-labelledby="debug-chat-tab" hidden={mode !== 'chat'}>
      <aside className="panel debug-settings"><div className="debug-panel-heading"><h3>{t('Request settings')}</h3></div><fieldset disabled={busy} className="debug-fields">
        <label className="form-field">{t('Model / route')}<SearchSelect label={t('Select model or route')} placeholder={t('Search models or routes…')} empty={t('No matching options')} options={options} value={target} disabled={busy} onChange={value => { setTarget(value); clear(); }}/></label>
        <label className="form-field">{t('API type')}<Select aria-label={t('API type')} searchable={false} disabled={busy} value={endpoint} onChange={e => { setEndpoint(e.target.value as typeof endpoint); clear(); }}><option value="chat/completions">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="messages">Anthropic Messages</option></Select></label>
        <div><div className="debug-parameter-heading"><strong>{t('Parameters')}</strong><button className="icon-action" aria-label={t('Add parameter')} onClick={() => setParameters([...parameters,{key:'',value:''}])}><Plus size={16}/></button></div>{parameters.map((p,index) => <div className="debug-parameter" key={index}><input aria-label={t('Parameter name')} placeholder="temperature" value={p.key} autoComplete="off" autoCapitalize="none" spellCheck={false} onChange={e => setParameters(parameters.map((p,i) => i === index ? {...p,key:e.target.value} : p))}/><input autoCapitalize="none" aria-label={t('Parameter value')} placeholder="0.2" value={p.value} autoComplete="off" onChange={e => setParameters(parameters.map((p,i) => i === index ? {...p,value:e.target.value} : p))}/><button className="icon-action" aria-label={t('Remove parameter')} onClick={() => setParameters(parameters.filter((_,i) => i !== index))}><X size={14}/></button></div>)}<small>JSON: true · 0.2 · {'"text"'} · {'{"key":"value"}'}</small></div>
        <small className="debug-settings-note">{t('Live tests may incur upstream costs. Clearing starts a new session.')}</small>
      </fieldset></aside>
      <section className="panel debug-chat">
        <div className="debug-panel-heading"><h3>{t('Conversation')}</h3><button type="button" className="button ghost small" disabled={busy} onClick={clear}><Trash2 size={14}/>{t('Clear conversation')}</button></div>
        <div ref={messageList} className="debug-messages" role="log" aria-label={t('Conversation')}>
          {!messages.length && <div className="debug-empty">{t('Select a model or route and send a message to start.')}</div>}
          {messages.map(message => <article className={'debug-message ' + message.role} key={message.id} aria-busy={message.status === 'pending' && message.role === 'assistant'}>
            {message.role === 'assistant' && message.status === 'pending' && <div className="debug-loading" role="status"><LoaderCircle size={16} className="import-spinner"/>{t(message.content ? 'Receiving response…' : message.reasoning ? 'Model is reasoning…' : 'Waiting for response…')}</div>}
            {message.content &&
              <div className={'debug-markdown' + (message.role === 'assistant' && message.status === 'error' ? ' route-error' : '')} role={message.role === 'assistant' && message.status === 'error' ? 'alert' : undefined}>
                <DebugMarkdown>{message.content}</DebugMarkdown>
              </div>}
            {message.warning && <p className="route-error" role="status">{t(message.warning)}</p>}
            {!!message.images?.length && <div className="debug-image-list">{message.images.map((src, index) => <img key={index} src={src} alt={t('Pasted image')} />)}</div>}
            {!!message.files?.length && <div className="debug-file-list">{message.files.map((file, index) => <span className="debug-file-chip" key={index} title={file.name}><FileText size={16}/><span>{file.name}</span></span>)}</div>}
            {message.role === 'user' && message.raw && <div className="debug-message-details"><DebugDetailButton title={t('Request JSON')}><pre className="debug-response">{message.raw}</pre></DebugDetailButton></div>}
            {message.role === 'assistant' && message.status !== 'pending' && <DebugMessageDetails message={message}/>}
          </article>)}
          {error && <div role="alert" className="route-error">{error}</div>}
        </div>
        <form className="debug-composer" onSubmit={e => { e.preventDefault(); void run(); }}>
          {(!!images.length || !!files.length) && <div className="debug-image-list debug-attachments">
            {images.map((src, index) => <div className="debug-attachment" key={index}><img src={src} alt={t('Pasted image')}/><button type="button" className="icon-action" disabled={busy || readingImages} aria-label={t('Remove image')} onClick={() => setImages(previous => previous.filter((_, i) => i !== index))}><X size={14}/></button></div>)}
            {files.map((file, index) => <div className="debug-file-chip" key={index} title={file.name}><FileText size={18}/><span>{file.name}<small>{Math.max(1, Math.ceil(file.size / 1024))} KB</small></span><button type="button" className="icon-action" disabled={busy || readingImages} aria-label={t('Remove attachment')} onClick={() => setFiles(previous => previous.filter((_, i) => i !== index))}><X size={14}/></button></div>)}
          </div>}
          <input ref={fileInput} type="file" hidden multiple disabled={busy || readingImages} onChange={e => { void addAttachments(Array.from(e.target.files ?? [])); e.target.value = ''; }}/>
          <button type="button" className="button ghost" disabled={busy || readingImages} aria-label={t('Add attachment')} title={t('Add attachment')} onClick={() => fileInput.current?.click()}>{readingImages ? <LoaderCircle size={18} className="import-spinner"/> : <Plus size={18}/>}</button>
          <textarea ref={composerInput} onPaste={e => {
            const pasted = Array.from(e.clipboardData.files);
            if (pasted.length) { e.preventDefault(); void addAttachments(pasted); }
          }} aria-label={t('Test prompt')} placeholder={t('Message… Enter to send, Shift+Enter for a new line')} rows={1} maxLength={8000} disabled={busy} autoComplete="off" autoCapitalize="none" spellCheck={false} value={prompt} onChange={e => setPrompt(e.target.value)} onKeyDown={e => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); void run(); } }}/>
          <button className="button primary" disabled={!valid || busy || readingImages} aria-label={t('Send test request')}><Send size={18}/></button>
        </form>
      </section>
    </div>
  </div>;
}
