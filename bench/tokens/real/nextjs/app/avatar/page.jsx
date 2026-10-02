// @feature upload
import { redirect } from 'next/navigation';
import { currentUser } from '@/lib/auth';
import AvatarForm from './form';

export const metadata = { title: 'Avatar' };

export default async function Avatar() {
  if (!(await currentUser())) redirect('/login');
  return <AvatarForm />;
}
