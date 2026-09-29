{@props rows: &[(&str, u32)], row: Snippet<&(&str, u32), usize>}
<table class="t">{#each rows as r, i}<tr>{@render row(r, i)}</tr>{/each}{@render children()}</table>
