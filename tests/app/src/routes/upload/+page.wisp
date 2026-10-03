<form method="post" enctype="multipart/form-data">
  <input aria-label="title" name="title" value={title}>
  <input aria-label="photo" type="file" name="photo">
  <button>Upload</button>
  <button formaction="?/export">Export</button>
</form>
{#if let Some(p) = problem}
  <p class="problem">{p}</p>
{/if}
{#if let Some(s) = saved}
  <p class="saved">{s.name}: {s.size} bytes of {s.kind}</p>
{/if}
