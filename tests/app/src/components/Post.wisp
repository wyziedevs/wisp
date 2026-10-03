{@props title: &str, date: &str = ""}
<article class="post">
  <h1>{title}</h1>
  <time>{date}</time>
  {@render children()}
</article>
