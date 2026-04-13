import path from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { cloudflare } from '@cloudflare/vite-plugin';

const repoRoot = path.resolve(__dirname, '..');

export default defineConfig({
  plugins: [
    react(),
    cloudflare({
      configPath: './wrangler.jsonc',
    }),
  ],
  publicDir: path.resolve(repoRoot, 'public'),
  resolve: {
    alias: {
      '@': path.resolve(repoRoot, 'src'),
      'next/navigation': path.resolve(__dirname, 'src/shims/next-navigation.ts'),
      'next/image': path.resolve(__dirname, 'src/shims/next-image.tsx'),
    },
  },
  server: {
    fs: {
      allow: [repoRoot],
    },
  },
});
