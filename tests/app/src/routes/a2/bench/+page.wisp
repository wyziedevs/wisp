<div class="bar">
  <button id="run" on:click="run(1000)">Create 1,000 rows</button>
  <button id="runlots" on:click="run(10000)">Create 10,000 rows</button>
  <button id="add" on:click="add()">Append 1,000 rows</button>
  <button id="update" on:click="update()">Update every 10th row</button>
  <button id="clear" on:click="rows = []">Clear</button>
  <button id="swaprows" on:click="swap()">Swap rows</button>
</div>
<table>
  <tbody id="tbody">
    {:#each rows as row (row.id)}
      <tr class:danger="selected === row.id">
        <td>{:row.id}</td><!-- wisp-ignore a11y-anchor-href -->
        <td><a class="lbl" on:click="selected = row.id">{:row.label}</a></td><!-- wisp-ignore a11y-anchor-href -->
        <td><a class="rm" on:click="rows.splice(rows.indexOf(row), 1)">x</a></td>
      </tr>
    {:/each}
  </tbody>
</table>

<script>
  // The js-framework-benchmark's keyed table.
  const A = ['pretty', 'large', 'big', 'small', 'tall', 'short', 'long', 'handsome', 'plain', 'quaint', 'clean', 'elegant', 'easy', 'angry', 'crazy', 'helpful', 'mushy', 'odd', 'unsightly', 'adorable', 'important', 'inexpensive', 'cheap', 'expensive', 'fancy']
  const C = ['red', 'yellow', 'blue', 'green', 'pink', 'brown', 'purple', 'brown', 'white', 'black', 'orange']
  const N = ['table', 'chair', 'house', 'bbq', 'desk', 'car', 'pony', 'cookie', 'sandwich', 'burger', 'pizza', 'mouse', 'keyboard']
  let rows = [], selected = null
  let id = 1
  const pick = (a) => a[Math.round(Math.random() * 1000) % a.length]
  const make = (n) => Array.from({ length: n }, () => ({ id: id++, label: `${pick(A)} ${pick(C)} ${pick(N)}` }))
  const run = (n) => (rows = make(n))
  const add = () => rows.push(...make(1000))
  function update() {
    for (let i = 0; i < rows.length; i += 10) rows[i].label += ' !!!'
  }
  function swap() {
    if (rows.length > 998) [rows[1], rows[998]] = [rows[998], rows[1]]
  }
</script>
