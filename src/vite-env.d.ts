/// <reference types="vite/client" />

interface Window {
  __ISOLATION_CHECK__?: {
    base: string;
    reload: boolean;
    loginMode: string | null;
    root: string;
    captureHoldMs?: number;
  };
}
