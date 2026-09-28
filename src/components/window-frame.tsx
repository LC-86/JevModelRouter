import { getCurrentWindow } from '@tauri-apps/api/window';
import { X, Minus, Maximize2 } from 'lucide-react';
import { usePreferences } from '../lib/preferences-context';

export function WindowFrame() {
  const { t } = usePreferences();
  if (!('__TAURI_INTERNALS__' in window)) return null;
  const win = getCurrentWindow();
  const directions = ['North', 'South', 'East', 'West', 'NorthEast', 'NorthWest', 'SouthEast', 'SouthWest'] as const;
  return <>
    <div className="window-drag" data-tauri-drag-region onDoubleClick={() => void win.toggleMaximize()}/>
    <div className="window-controls">
      <button className="window-dot close" aria-label={t('Close')} onClick={() => void win.close()}><X/></button>
      <button className="window-dot minimize" aria-label={t('Minimize')} onClick={() => void win.minimize()}><Minus/></button>
      <button className="window-dot maximize" aria-label={t('Maximize or restore')} onClick={() => void win.toggleMaximize()}><Maximize2/></button>
    </div>
    {directions.map(direction => <div key={direction} className={'window-resize ' + direction} onMouseDown={e => { if (e.button === 0) void win.startResizeDragging(direction); }}/>) }
  </>;
}
