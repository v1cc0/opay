import path from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { cloudflare } from '@cloudflare/vite-plugin';

export default defineConfig({
  plugins: [
    react(),
    cloudflare({
      configPath: './wrangler.jsonc',
    }),
  ],
  publicDir: path.resolve(__dirname, 'public'),
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
      'next/navigation': path.resolve(__dirname, 'src/shims/next-navigation.ts'),
      'next/image': path.resolve(__dirname, 'src/shims/next-image.tsx'),
    },
  },
});
