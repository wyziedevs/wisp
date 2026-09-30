// the-benchmarker's GET /, as its Next.js entry answers it.
export const dynamic = 'force-dynamic';

export function GET() {
    return new Response(null);
}
