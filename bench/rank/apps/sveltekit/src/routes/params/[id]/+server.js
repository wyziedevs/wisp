import { text } from '@sveltejs/kit';
export const GET = ({ params, url, cookies }) => text(`id=${params.id} q=${url.searchParams.get('q')} sid=${cookies.get('sid') ?? 'none'}`);
