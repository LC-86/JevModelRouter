import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { ChevronRight, X } from 'lucide-react';
import { usePreferences } from '../lib/preferences-context';

function DebugDetailDialog({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  const { t } = usePreferences();
  const dialog = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    return () => { element.close(); previous?.focus(); };
  }, []);
  return createPortal(<dialog ref={dialog} className="dialog debug-detail-dialog" aria-labelledby={titleId} onCancel={event => { event.preventDefault(); onClose(); }} onClick={event => {
    if (event.target !== event.currentTarget) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose();
  }}>
    <div className="debug-detail-dialog-header"><h2 id={titleId}>{title}</h2><button autoFocus type="button" className="icon-action" aria-label={t('Close')} onClick={onClose}><X size={20}/></button></div>
    <div className="debug-detail-dialog-body">{children}</div>
  </dialog>, document.body);
}

export function DebugDetailButton({ title, children }: { title: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return <>
    <button type="button" className="debug-detail-button" aria-haspopup="dialog" onClick={() => setOpen(true)}><ChevronRight size={14}/>{title}</button>
    {open && <DebugDetailDialog title={title} onClose={() => setOpen(false)}>{children}</DebugDetailDialog>}
  </>;
}
