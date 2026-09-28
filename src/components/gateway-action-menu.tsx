import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { LoaderCircle, Pause, Power } from 'lucide-react';
import { usePreferences } from '../lib/preferences-context';

export function GatewayActionMenu({ anchor, busy, onClose, onConfirm }: { anchor: RefObject<HTMLButtonElement | null>; busy: boolean; onClose: () => void; onConfirm: (pause: boolean) => void }) {
  const { t } = usePreferences();
  const menu = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: 0, top: 0 });
  const [pending, setPending] = useState<boolean | null>(null);
  useLayoutEffect(() => {
    const place = () => {
      const button = anchor.current?.getBoundingClientRect();
      const panel = menu.current?.getBoundingClientRect();
      if (!button || !panel) return;
      setPosition({ left: Math.max(8, Math.min(button.right + 10, window.innerWidth - panel.width - 8)), top: Math.max(8, Math.min(button.top, window.innerHeight - panel.height - 8)) });
    };
    place(); window.addEventListener('resize', place); window.addEventListener('scroll', place, true);
    menu.current?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => { window.removeEventListener('resize', place); window.removeEventListener('scroll', place, true); };
  }, [anchor]);
  useEffect(() => {
    const close = (event: PointerEvent) => {
      if (!busy && !menu.current?.contains(event.target as Node) && !anchor.current?.contains(event.target as Node)) onClose();
    };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [anchor, busy, onClose]);
  return createPortal(<div ref={menu} id="gateway-action-menu" className="gateway-action-menu" style={position} role="menu" aria-label={t('Stop gateway')} aria-busy={busy} onKeyDown={event => {
    if (event.key === 'Escape') { event.preventDefault(); if (!busy) { onClose(); anchor.current?.focus(); } }
    if (event.key === 'Tab' && !busy) onClose();
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault(); const items = Array.from(menu.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []);
      const current = items.indexOf(document.activeElement as HTMLButtonElement);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (current + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      items[next]?.focus();
    }
  }}>
    {[true, false].map(pause => <button key={String(pause)} type="button" role="menuitem" disabled={busy} onClick={() => { setPending(pause); onConfirm(pause); }}>
      {busy && pending === pause ? <LoaderCircle size={17} className="import-spinner"/> : pause ? <Pause size={17}/> : <Power size={17}/>}
      <span><strong>{t(pause ? 'Pause service' : 'Stop and restore configurations')}</strong><small>{t(pause ? 'Keep agent configuration; new requests return HTTP 404.' : 'Restore agent configurations and close the gateway.')}</small></span>
    </button>)}
  </div>, document.body);
}
