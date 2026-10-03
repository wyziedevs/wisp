<h1>Snap</h1>
<form>
  <input id="text" name="text" aria-label="Text">
  <input id="secret" name="secret" type="password" aria-label="Secret">
  <input id="off" name="off" autocomplete="off" aria-label="Off">
  <input id="box" name="box" type="checkbox" aria-label="Box">
  <select id="pick" name="pick" aria-label="Pick"><option>a</option><option>b</option></select>
  <textarea id="note" name="note" aria-label="Note"></textarea>
</form>
<button id="more" on:click="n++">More {:n}</button>
<a id="away" href="/a2/islands">Away</a>

<script>
  let n = 0
  export const snapshot = { capture: () => n, restore: (v) => (n = v) }
</script>
