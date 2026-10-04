import { sveltekit } from '@sveltejs/kit/vite';
import node from '@sveltejs/adapter-node';
import cloudflare from '@sveltejs/adapter-cloudflare';
// ADAPTER=cloudflare vite build, or the default, node.
export default { plugins: [sveltekit({ adapter: process.env.ADAPTER === 'cloudflare' ? cloudflare() : node() })] };
