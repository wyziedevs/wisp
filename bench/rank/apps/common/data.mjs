// The data every app serves, so the output is the same in each framework.
export const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
export const items = (n) => Array.from({ length: n }, (_, i) => `Item <${i + 1}> & co`);
export const page = (xs) => `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>List</title></head><body><h1>List</h1><ul>${xs.map((s) => `<li>${esc(s)}</li>`).join('')}</ul></body></html>`;
export const rows = () => Array.from({ length: 200 }, (_, k) => ({ id: k + 1, name: `user-${k + 1}`, active: (k + 1) % 3 !== 0, score: ((k + 1) * 37) % 101, tags: ['a', `t${(k + 1) % 7}`] }));
export const cookie = (h, k) => h?.match(new RegExp(`(?:^|; )${k}=([^;]*)`))?.[1];
