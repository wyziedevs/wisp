{@props title, count: u32 = 0, featured: bool = false, row: Snippet<&Post, usize>}
<h2>{title}{#if featured} ★{/if}</h2>{@render children()}
