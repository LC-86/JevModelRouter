import { type ReactNode, useEffect, useId, useRef, useState } from 'react';
import { Check, ChevronDown, Search } from 'lucide-react';

export interface SelectOption { value: string; label: string; group: string; keywords?: string; icon?: ReactNode }

export function SearchSelect({ value, options, onChange, label, placeholder, empty, disabled = false, id: controlId, searchable = true, values, onToggle, bulkActions, selectionControls }: {
  values?: string[]; onToggle?: (value: string) => void; bulkActions?: ReactNode; selectionControls?: ReactNode;
  value: string; options: SelectOption[]; onChange: (value: string) => void;
  label: string; placeholder: string; empty: string; disabled?: boolean; id?: string; searchable?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const id = useId();
  const filtered = options.filter((option) => `${option.label} ${option.keywords || ''} ${option.group}`.toLowerCase().includes(query.trim().toLowerCase()));
  const selected = options.find((option) => option.value === value);
  const groups = [...new Set(filtered.map((option) => option.group))];
  useEffect(() => {
    if (!open) return;
    if (searchable) input.current?.focus();
    else list.current?.focus();
    const close = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [open, searchable]);
  useEffect(() => {
    if (!open) return;
    const container = list.current;
    const option = document.getElementById(`${id}-${active}`);
    if (!container || !option) return;
    // Scroll options only; scrollIntoView also moves/clips ancestor form panels.
    const bounds = container.getBoundingClientRect();
    const item = option.getBoundingClientRect();
    if (item.top < bounds.top) container.scrollTop -= bounds.top - item.top;
    else if (item.bottom > bounds.bottom) container.scrollTop += item.bottom - bounds.bottom;
  }, [active, open, id]);
  const choose = (selected: string) => { if (onToggle) { onToggle(selected); return; } onChange(selected); setOpen(false); trigger.current?.focus(); };
  return <div ref={root} className="search-select" onBlur={(event) => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false);
  }} onKeyDown={(event) => {
    if (event.key === 'Escape' && open) { event.preventDefault(); event.stopPropagation(); setOpen(false); trigger.current?.focus(); }
  }}>
    <button id={controlId} ref={trigger} type="button" className="search-select-trigger" disabled={disabled} aria-label={label} aria-haspopup="listbox" aria-expanded={open} aria-controls={`${id}-list`} onClick={() => { setQuery(''); setActive(0); setOpen(!open); }} onKeyDown={(event) => {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setQuery(''); setActive(0); setOpen(true); }
    }}><span className="search-select-label">{values ? (values.length ? `${label} (${values.length})` : label) : <>{selected?.icon}<span className="search-select-text">{selected?.label || label}</span></>}</span><ChevronDown size={15} /></button>
    {open && <div className="search-select-popover">
      {searchable && <div className="search-select-search"><Search size={15} /><input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} ref={input} role="combobox" aria-label={placeholder} aria-expanded="true" aria-autocomplete="list" aria-controls={`${id}-list`} aria-activedescendant={filtered[active] ? `${id}-${active}` : undefined} placeholder={placeholder} value={query} onChange={(event) => { setQuery(event.target.value); setActive(0); }} onKeyDown={(event) => {
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setActive((index) => Math.max(0, Math.min(filtered.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))); }
        if (event.key === 'Enter') { event.preventDefault(); if (filtered[active]) choose(filtered[active].value); }
      }} /></div>}
      {values && bulkActions && <div className="search-select-bulk-actions">{bulkActions}</div>}
      {selectionControls}
      <div ref={list} tabIndex={searchable ? undefined : -1} onKeyDown={(event) => {
        if (searchable) return;
        if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); setActive((index) => Math.max(0, Math.min(filtered.length - 1, index + (event.key === 'ArrowDown' ? 1 : -1)))); }
        if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); if (filtered[active]) choose(filtered[active].value); }
      }} aria-activedescendant={!searchable && filtered[active] ? `${id}-${active}` : undefined} id={`${id}-list`} role="listbox" aria-multiselectable={values ? true : undefined} aria-label={label} className="search-select-options">
        {groups.map((group) => <div key={group} role="group" aria-label={group}>{group && <div className="search-select-group">{group}</div>}{filtered.filter((option) => option.group === group).map((option) => {
          const index = filtered.indexOf(option);
          const checked = values ? values.includes(option.value) : value === option.value;
          return <div key={option.value} id={`${id}-${index}`} role="option" aria-selected={checked} className={`search-select-option${active === index ? ' active' : ''}`} onPointerMove={() => setActive(index)} onMouseDown={(event) => event.preventDefault()} onClick={(event) => { event.preventDefault(); event.stopPropagation(); choose(option.value); }}><span className="search-select-label" title={option.label}>{option.icon}<span className="search-select-text">{option.label}</span></span><span className={`search-select-check${values ? ' multiple' : ''}`} aria-hidden="true">{checked && <Check size={14} />}</span></div>;
        })}</div>)}
        {!filtered.length && <div className="search-select-empty">{empty}</div>}
      </div>
    </div>}
  </div>;
}
