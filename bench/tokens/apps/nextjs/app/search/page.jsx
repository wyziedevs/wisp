// @feature search
import { items } from '@/lib/db';
import Search from './search';

export const metadata = { title: 'Search' };

export default async function Page() {
  return <Search items={await items()} />;
}
