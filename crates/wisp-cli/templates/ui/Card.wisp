{@props title: Option<&str> = None}
<article class="card">
  {#if let Some(title) = title}<h2>{title}</h2>{/if}
  {@render children()}
</article>

<style>
  .card {
    display: grid;
    gap: 0.75rem;
    padding: 1.25rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: calc(var(--radius, 0.375rem) * 2);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
  }
  h2 {
    margin: 0;
    font-size: 1.125rem;
    font-weight: 600;
    line-height: 1.3;
  }
</style>
