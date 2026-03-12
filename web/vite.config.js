import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  server: {
    port: 5800,
    strictPort: false,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
