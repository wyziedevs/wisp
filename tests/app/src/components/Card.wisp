{@props title: &str, count: u32 = 0, featured: bool = false}
<section class="card">
  <h2>{title}{#if featured} ★{/if}</h2>
  <p>{count} items</p>
  {@render children()}
</section>
