<h1>Blog</h1>
<ul>
  {#each wisp::pages("blog") as p}
    <li><a href={p.path}>{p.title}</a> {p.get("date")}</li>
  {/each}
</ul>
