import { people } from '../../../../../page.mjs';

export function load() {
    return { people: people() };
}
