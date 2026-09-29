import { load } from '../../../fortunes.mjs';

// Render on every request, as the other servers do, instead of once at build.
export const dynamic = 'force-dynamic';
export const metadata = { title: 'Fortunes' };

export default function Fortunes() {
    return (
        <table>
            <tbody>
                <tr><th>id</th><th>message</th></tr>
                {load().map((f) => (
                    <tr key={f.id}><td>{f.id}</td><td>{f.message}</td></tr>
                ))}
            </tbody>
        </table>
    );
}
