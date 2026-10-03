{@element "x-card"}
{@props title: &str, count: u32 = 2, featured: bool = false, tags: Vec<String> = Vec::new()}
<h2>{:title}{:#if featured} ★{:/if}</h2>
<button on:click="count++">{:count}</button>
<p class="tags">{:tags.join(', ')}</p>
<Badge text={:title} />
{@render children()}

<style>
  h2 { color: rgb(0, 128, 0) }
</style>
