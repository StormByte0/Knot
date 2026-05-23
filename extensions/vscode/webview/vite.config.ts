import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: path.resolve(__dirname, '../media/storymap'),
    emptyOutDir: true,
    rollupOptions: {
      output: {
        entryFileNames: 'storymap.js',
        assetFileNames: 'storymap.[ext]',
      },
    },
  },
});
