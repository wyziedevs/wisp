{@props title: &str, count: u32 = 0, featured: bool = false}
<h2>{title}{#if featured} ★{/if} ({count})</h2>{@render children()}
