// @feature layout
import Link from 'next/link';

export default function RootLayout({ children }) {
  return (
    <html lang="en">
      <body>
        <nav>
          <Link href="/">Items</Link>
          <Link href="/contact">Contact</Link>
          <Link href="/search">Search</Link>
        </nav>
        {children}
      </body>
    </html>
  );
}
