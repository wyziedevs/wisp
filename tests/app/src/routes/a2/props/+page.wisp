<Chip label="rust" />
<div id="spread">{:#each pills as p (p.label)}<Pill {:...p} on={:p.label === picked} />{:/each}</div>
<Pill {:...one} title="after" />
<ul id="keys">{:#each rows as r (r.id)}<li>{:r.id}</li>{:/each}</ul>
<button id="rekey" on:click="rows[0].id = 9">Rekey</button>
{:#try}
  <p id="risky">{:risky()}</p>
{:catch e}
  <button id="retry" on:click="broken = false; reset()">Retry {:e.message}</button>
{:/try}
<button id="break" on:click="broken = true">Break</button>

<script>
  let pills = [{ label: 'a', tone: 'warm' }, { label: 'b', title: 'bee' }]
  let picked = 'b'
  let one = { label: 'one', tone: 'cool', title: 'before' }
  let rows = [{ id: 1 }, { id: 2 }]
  let broken = false
  const risky = () => {
    if (broken) throw new Error('broke')
    return 'fine'
  }
</script>
