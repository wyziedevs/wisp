import type { PageProps } from "fresh";

export default function App({ Component }: PageProps) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>List</title>
      </head>
      <body>
        <Component />
      </body>
    </html>
  );
}
