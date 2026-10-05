export const dynamic = 'force-dynamic';
export default function List() {
  const items = Array.from({ length: 1000 }, (_, i) => `Item <${i + 1}> & co`);
  return (
    <>
      <h1>List</h1>
      <ul>
        {items.map((s) => (
          <li key={s}>{s}</li>
        ))}
      </ul>
    </>
  );
}
