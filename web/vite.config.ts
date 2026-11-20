import { defineConfig } from 'vite'

// JSX goes to Preact via tsconfig's jsxImportSource; no framework plugin needed.
// Relative base so the build works under the GitHub Pages project path.
export default defineConfig({
  base: './',
})
