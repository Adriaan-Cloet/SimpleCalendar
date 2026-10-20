/// <reference types="vitest/config" />
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) =>
					filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// SPA mode: no prerendered pages, every route falls back to index.html
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	// Keep Rust compiler errors visible in the terminal
	clearScreen: false,
	server: {
		// Must match devUrl in src-tauri/tauri.conf.json
		port: 5173,
		strictPort: true,
		watch: {
			ignored: ['**/src-tauri/**']
		}
	},
	// Unit tests for pure functions in src/lib (docs/architecture.md §12). No DOM emulation.
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
