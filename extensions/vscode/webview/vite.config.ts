import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  css: {
    postcss: {},
  },
  build: {
    // import.meta.dirname (not __dirname): the config is ESM ("type":
    // "module") and Vite's planned default `configLoader: 'native'` rejects
    // CJS globals. Requires Node >= 20.11 — the extension pins 22 via .nvmrc.
    outDir: path.resolve(import.meta.dirname, '../media/storymap'),
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: 'storymap.js',
        assetFileNames: 'storymap.[ext]',
      },
    },
  },
});
