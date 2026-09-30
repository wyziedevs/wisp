import { people } from '../../../../page.mjs';

// Render on every request, as the other servers do, instead of once at build.
export const dynamic = 'force-dynamic';
export const metadata = { title: 'Roster' };

export default function Roster() {
    return (
        <>
            <h1>Roster</h1>
            <table>
                <thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>
                <tbody>
                    {people().map((p) => (
                        <tr key={p.id} className={p.active ? 'on' : 'off'}><td>{p.id}</td><td>{p.name}</td><td>{p.score}</td></tr>
                    ))}
                </tbody>
            </table>
            <form method="post" action="/subscribe"><label>Email <input type="email" name="email" required /></label><button>Subscribe</button></form>
        </>
    );
}
