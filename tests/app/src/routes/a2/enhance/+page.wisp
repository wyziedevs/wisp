<form method="post" action="?/add" use:enhance="submit">
  <input name="text" bind:value="text">
  <button id="send">Send</button>
</form>
<p id="pending">{:pending}</p>
<p id="status">{:status}</p>
<ul id="items">
  {#each items as item}<li>{item}</li>{/each}
  {:#each optimistic as o}<li class="opt">{:o}</li>{:/each}
</ul>
<form method="post" action="?/answer" use:enhance="json"><button id="json">JSON</button></form>
<p id="answer">{:answer}</p>

<script>
  let text = ''
  let pending = false
  let status = ''
  let optimistic = []
  let answer = ''
  function submit({ formData }) {
    pending = true
    optimistic = [formData.get('text')]
    return (r) => {
      pending = false
      status = r.status
      optimistic = []
    }
  }
  function json() {
    return (r) => {
      answer = r.data.n
    }
  }
</script>
