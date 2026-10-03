import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-node';
export default { plugins: [sveltekit({ adapter: adapter() })] };
