// the-benchmarker's GET / (SvelteKit has no entry there): an empty 200.
export function GET() {
    return new Response();
}
