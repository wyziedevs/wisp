import { load as fortunes } from '../../../../fortunes.mjs';

export function load() {
    return { fortunes: fortunes() };
}
