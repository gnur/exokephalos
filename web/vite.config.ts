import { readFileSync } from 'node:fs';
import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';
import { VitePWA } from 'vite-plugin-pwa';

export default defineConfig(({ mode }) => {
  const xoVersion = loadEnv(mode, '..', '').XO_VERSION?.trim() || 'dev';
  return {
    plugins: [
      react(),
      {
        name: 'xo-installer',
        generateBundle() {
          this.emitFile({
            type: 'asset',
            fileName: 'install.sh',
            source: readFileSync('../install.sh', 'utf8'),
          });
        },
      },
      {
        name: 'xo-version-manifest',
        generateBundle() {
          this.emitFile({
            type: 'asset',
            fileName: 'version.json',
            source: `${JSON.stringify({ version: xoVersion })}\n`,
          });
        },
      },
      VitePWA({
        registerType: 'prompt',
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
    define: {
      __XO_VERSION__: JSON.stringify(xoVersion),
    },
    build: {
      target: 'es2022',
      outDir: 'dist',
      emptyOutDir: true,
      sourcemap: false,
    },
    worker: { format: 'es' },
  };
});
