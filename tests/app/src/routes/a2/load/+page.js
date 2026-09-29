import { label } from '$lib/names.js'

export async function load({ data, url }) {
  return { from: data.server + '+browser', q: url.searchParams.get('q'), shout: label('hi') }
}
