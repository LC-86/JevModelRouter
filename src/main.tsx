import React from 'react';
import ReactDOM from 'react-dom/client';
import '@fontsource-variable/manrope';
import '@fontsource/ibm-plex-mono/400.css';
import './styles.css';
import App from './App';
import { PreferencesProvider } from './lib/preferences-context';

if ('__TAURI_INTERNALS__' in window) document.documentElement.classList.add('desktop-window');

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <PreferencesProvider><App /></PreferencesProvider>
  </React.StrictMode>,
);
