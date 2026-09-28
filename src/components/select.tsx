import { Children, isValidElement, type ReactNode } from 'react';
import { SearchSelect } from './search-select';
import { usePreferences } from '../lib/preferences-context';

// Keep option-based forms while sharing the searchable selector's rendering and keyboard behavior.
export function Select({ children, value, onChange, required, disabled, id, 'aria-label': ariaLabel, searchable = true, renderIcon }: {
  children: ReactNode; value: string; onChange: (event: { target: { value: string } }) => void;
  required?: boolean; disabled?: boolean; id?: string; 'aria-label'?: string; searchable?: boolean; renderIcon?: (value: string) => ReactNode;
}) {
  const { t } = usePreferences();
  const options = Children.toArray(children).flatMap((child) => {
    if (!isValidElement<{ value: string; children: ReactNode }>(child)) return [];
    return [{ value: child.props.value, label: String(child.props.children), group: '', icon: renderIcon?.(child.props.value) }];
  });
  return <div className="select-control">
    <SearchSelect searchable={searchable} id={id} value={value} options={options} onChange={(next) => onChange({ target: { value: next } })} label={ariaLabel || t('Select option')} placeholder={t('Search options…')} empty={t('No matching options')} disabled={disabled} />
    {required && <input autoComplete="off" autoCapitalize="none" autoCorrect="off" spellCheck={false} className="select-validation" tabIndex={-1} aria-hidden="true" value={value} required disabled={disabled} onChange={() => {}} onInvalid={(event) => {
      event.preventDefault();
      event.currentTarget.parentElement?.querySelector('button')?.focus();
    }} />}
  </div>;
}
