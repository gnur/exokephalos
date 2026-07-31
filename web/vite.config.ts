import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      registerType: 'autoUpdate',
      strategies: 'generateSW',
      includeAssets: ['logo.svg', 'icons/*.png'],
      manifest: {
        id: '/',
        name: 'xo',
        short_name: 'xo',
        description: 'Private, offline-first knowledge workspace',
        theme_color: '#08111f',
        background_color: '#08111f',
        display: 'standalone',
        start_url: '/',
        scope: '/',
        icons: [
          { src: '/icons/xo-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/icons/xo-512.png', sizes: '512x512', type: 'image/png' },
          {
            src: '/icons/xo-512-maskable.png',
            sizes: '512x512',
            type: 'image/png',
            purpose: 'maskable',
          },
        ],
      },
      workbox: {
        navigateFallback: '/index.html',
        globPatterns: ['**/*.{js,css,html,wasm,svg,png,webmanifest}'],
        cleanupOutdatedCaches: true,
        maximumFileSizeToCacheInBytes: 12 * 1024 * 1024,
      },
      devOptions: { enabled: true },
    }),
  ],
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
    sourcemap: false,
  },
  worker: { format: 'es' },
});
