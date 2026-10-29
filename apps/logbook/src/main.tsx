import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './index.css';

// The system's theme, followed as it changes (index.css's `.dark`).
const dark = window.matchMedia('(prefers-color-scheme: dark)');
const theme = () => document.documentElement.classList.toggle('dark', dark.matches);
theme();
dark.addEventListener('change', theme);

// No browser menu on a right click (Reload, Inspect Element): an app has
// none. Where text is typed or selected it stays, for copy and paste and
// the spelling; in `tauri dev` too, for the inspector.
document.addEventListener('contextmenu', (e) => {
  if (import.meta.env.DEV) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest('input, textarea, [contenteditable="true"]')) return;
  if (window.getSelection()?.toString()) return;
  e.preventDefault();
});

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
