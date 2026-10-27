import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Tauri loads the dev server from this port (src-tauri/tauri.conf.json):
// 5176, as brainy's web app keeps 5173, Galley's 5174 and Galley's desktop 5175.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { port: 5176, strictPort: true },
  build: { target: 'es2022' },
});
