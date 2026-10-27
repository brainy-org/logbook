import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.js';
import './index.css';

// The system's theme, followed as it changes (index.css's `.dark`).
const dark = window.matchMedia('(prefers-color-scheme: dark)');
const theme = () => document.documentElement.classList.toggle('dark', dark.matches);
theme();
dark.addEventListener('change', theme);

const root = document.getElementById('root');
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
