// @feature live
'use client';
import { useRouter } from 'next/navigation';
import { useEffect } from 'react';

export default function Live() {
  const router = useRouter();
  useEffect(() => {
    const events = new EventSource('/posts/events');
    events.onmessage = () => router.refresh();
    return () => events.close();
  }, [router]);
  return null;
}
