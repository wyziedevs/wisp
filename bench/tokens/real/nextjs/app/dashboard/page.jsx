// @feature auth
import Link from 'next/link';
import { redirect } from 'next/navigation';
import { currentUser } from '@/lib/auth';
import Details from '@/components/details';
import { logout } from '../actions';

export const metadata = { title: 'Dashboard' };

export default async function Dashboard() {
  const user = await currentUser();
  if (!user) redirect('/login');
  return (
    <>
      <p>Signed in as {user.email}</p>
      <form action={logout}>
        <button>Log out</button>
      </form>
      {/* @feature upload */}
      {user.avatar && <img src={`/avatars/${user.id}`} alt="Avatar" />}
      <Link href="/avatar">Change avatar</Link>
      {/* @feature component */}
      <Details title="Account">
        <p>Signed up with {user.email}</p>
      </Details>
    </>
  );
}
