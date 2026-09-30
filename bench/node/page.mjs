// /page, shared by the Node and Bun servers: 50 rows built per request with
// a name to escape and a class chosen by a boolean. SvelteKit and Next.js
// render `people()` with their own components; the rest use `renderPage()`,
// the layout, table and form in a template literal, as fortunes.mjs does.
import { escape } from './fortunes.mjs';

const names = ['Ada <&"', 'Alan <&"', 'Grace <&"', 'Linus <&"', 'Edsger <&"'];

export function people() {
    return Array.from({ length: 50 }, (_, i) => {
        const id = i + 1;
        return { id, name: names[id % 5], score: (id * 37) % 101, active: id % 3 !== 0 };
    });
}

export function renderPage() {
    const rows = people()
        .map((p) => `<tr class="${p.active ? 'on' : 'off'}"><td>${p.id}</td><td>${escape(p.name)}</td><td>${p.score}</td></tr>`)
        .join('\n');
    return `<!DOCTYPE html>
<html>
<head><title>Roster</title></head>
<body>
<header><nav><a href="/">Home</a><a href="/page">Roster</a><a href="/about">About</a></nav></header>
<main>
<h1>Roster</h1>
<table>
<thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>
<tbody>
${rows}
</tbody>
</table>
<form method="post" action="/subscribe"><label>Email <input type="email" name="email" required></label><button>Subscribe</button></form>
</main>
<footer><p>Built with the framework under test.</p></footer>
</body>
</html>
`;
}
