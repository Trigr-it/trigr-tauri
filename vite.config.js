import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import keyfireDevBridge from './scripts/vite-dev-bridge.mjs';

export default defineConfig({
  plugins: [react(), keyfireDevBridge()], // dev bridge is serve-only (see scripts/vite-dev-bridge.mjs)
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/target/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'es2021',
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    outDir: 'build',
  },
});
