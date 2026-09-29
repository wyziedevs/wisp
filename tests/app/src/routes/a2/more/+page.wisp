<wisp:window bind:innerWidth on:keydown.k="keys++" />
<p id="width">{:innerWidth > 0}</p>
<p id="keys">{:keys}</p>

<Pill label="served" tone="warm" title="tip" />
<div id="pills">
  {:#each tags as tag (tag)}<Pill label={:tag} on={:tag === picked} title={:tag} />{:/each}
</div>

{:#key version}<p id="keyed">v{:version}</p>{:/key}
<button id="bump" on:click="version++">Bump</button>

{:#await slow}
  <p id="wait">Loading</p>
{:then v}
  <p id="wait">Got {:v}</p>
{:catch e}
  <p id="wait">Failed {:e.message}</p>
{:/await}
{:#await fails then v}<p>{:v}</p>{:catch e}<p id="failed">{:e}</p>{:/await}

{:#try}
  <p id="tried">{:risky()}</p>
{:catch e}
  <p id="caught">Caught {:e.message}</p>
{:/try}
<button id="break" on:click="broken = true">Break</button>

<div id="group">
  <input type="radio" name="size" value="s" bind:group="size"> <input type="radio" name="size" value="l" bind:group="size">
  <input type="checkbox" name="extra" value="a" bind:group="extras"> <input type="checkbox" name="extra" value="b" bind:group="extras">
</div>
<p id="picked">{:size} {:extras.join('+')}</p>
<div id="edit" contenteditable bind:innerHTML="html"></div>
<p id="html">{:html}</p>
<div id="box" bind:clientWidth="boxw" style="width: 120px">box</div>
<p id="boxw">{:boxw}</p>
<p id="styled" style={:{ color: hue, fontWeight: 700 }} {:...attrs}>styled</p>

<wisp:element this={:tag} id="dyn">dynamic</wisp:element>
<button id="retag" on:click="tag = tag == 'h2' ? 'h3' : 'h2'">Retag</button>

<p id="map">{:m.get('a')} {:m.size} {:s.has(2)}</p>
<button id="mapset" on:click="m.set('a', m.get('a') + 1); s.add(2)">Map</button>
<p id="todo">{:todo.done} {:todo.label}</p>
<button id="toggle" on:click="todo.toggle()">Toggle</button>
<p id="ctx">{:theme}</p>
<div id="lazy" client:visible><button id="lazyb" on:click="lazy++">{:lazy}</button></div>
<div id="touch" client:interaction><button id="touchb" on:click="lazy++">{:lazy}</button></div>
<div id="port" use:portal="'#target'">ported</div>
<div id="target"></div>
{:#if shown}<p id="fx" in:fade out:spin>fx</p>{:/if}
<button id="show" on:click="shown = !shown">Show</button>
{:#each 3 as i}<i class="n">{:i}</i>{:/each}

<script>
  let keys = 0
  let innerWidth
  let tags = ['a', 'b']
  let picked = 'b'
  let version = 1
  let slow = new Promise((ok) => setTimeout(() => ok('it'), 50))
  let fails = Promise.reject('nope')
  let broken = false
  const risky = () => {
    if (broken) throw new Error('broke')
    return 'fine'
  }
  let size = 's'
  let extras = ['b']
  let html = '<b>bold</b>'
  let boxw = 0
  let hue = 'red'
  let attrs = { 'data-x': 1, title: 'spread' }
  let tag = 'h2'
  let m = new Map([['a', 1]])
  let s = new Set([1])
  class Todo {
    done = $state(false)
    label = $derived(this.done ? 'done' : 'open')
    toggle() {
      this.done = !this.done
    }
  }
  const todo = new Todo()
  const [getTheme, setTheme] = context()
  setTheme('dark')
  const theme = getTheme()
  let lazy = 0
  let shown = false
  function spin(el, o, { direction }) {
    return { duration: 50, css: (t) => `transform: rotate(${(1 - t) * 90}deg); opacity: ${t}` }
  }
</script>
