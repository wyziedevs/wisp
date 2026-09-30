export default function SiteLayout({ children }) {
    return (
        <>
            <header><nav><a href="/">Home</a><a href="/page">Roster</a><a href="/about">About</a></nav></header>
            <main>{children}</main>
            <footer><p>Built with the framework under test.</p></footer>
        </>
    );
}
