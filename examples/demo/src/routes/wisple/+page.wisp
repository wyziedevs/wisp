<wisp:head>
  <title>Wisple</title>
  <meta name="description" content="A word game in the style of Wordle, written with Wisp">
</wisp:head>

<h1 class="visually-hidden">Wisple</h1>

<!-- With JavaScript, letters are typed here and only a finished guess is
     posted. Without it, each key is its own form post, which works too. -->
<script>
  let guess = data.guess
  let pressed = ''
  let form, grid, enter
  const POP = [{ scale: 1.12 }, { scale: 1 }]
  const SHAKE = { translate: ['0', '-6px', '6px', '-4px', '4px', '0'] }

  // The server has a new row: start a fresh guess.
  effect(() => { guess = data.guess }, () => [data.tries])

  function type(key) {
    if (data.over) return
    guess = key === 'backspace' ? guess.slice(0, -1) : (guess + key).slice(0, 5)
  }

  function press(e) {
    if (e.ctrlKey || e.metaKey || e.altKey || form.hasAttribute('aria-busy')) return
    const key = e.key.toLowerCase()
    if (key === 'enter') {
      e.preventDefault()
      if (enter.disabled) grid.querySelector('.current')?.animate(SHAKE, 300)
      else form.requestSubmit(enter)
    } else if (key === 'backspace' || /^[a-z]$/.test(key)) {
      e.preventDefault()
      pressed = key // the on-screen key dips too
      setTimeout(() => (pressed = ''), 120)
      type(key)
    }
  }

  const pop = (tile) => ({ update: (letter) => letter && tile.animate(POP, 150) })
</script>

<form class="wisple" method="post" action="?/enter" bind:this="form" on:keydown.window="press">
  <a class="how-to-play" href="/wisple/how-to-play">How to play</a>

  <div class="grid" class:won={won} bind:this="grid">
    {#each rows as row, r}
      <h2 class="visually-hidden">Row {r + 1}</h2>
      <div class="row {row.class}">
        {#each row.tiles as tile, i}
          <div class="letter {tile.mark.class()}" style="--i: {i}" use:pop="row.class === 'current' && guess[i]">
            <span :text="row.class === 'current' ? guess[i] ?? '' : tile.letter">{tile.letter}</span>
            <span class="visually-hidden">{tile.mark.label()}</span>
          </div>
        {/each}
      </div>
    {/each}
  </div>

  {#if won}
    <!-- A flock of little ghosts rises past the board. -->
    <div class="wisps" aria-hidden="true">
      {#each 0..16 as i}
        <span style="--i: {i}"></span>
      {/each}
    </div>
  {/if}

  <input type="hidden" name="guess" value={guess} bind:value="guess">

  <div class="controls">
    {#if over}
      <p class="result" role="status">
        {#if won}
          You got it in {tries}!
        {:else}
          The word was <strong>{answer}</strong>.
        {/if}
      </p>
      <button class="button primary restart" formaction="?/restart" bind:this="enter">Play again</button>
    {:else}
      <div class="keyboard">
        {#each keys as row, r}
          <div class="row">
            {#if r == 2}
              <button class="wide enter" disabled={!full} :disabled="guess.length < 5" bind:this="enter">Enter</button>
            {/if}
            {#each row as key}
              <button class={key.mark.class()} formaction="?/update" name="key" value={key.letter}
                      aria-label="{key.letter} {key.mark.label()}" disabled={full}
                      :disabled="guess.length === 5" class:pressed="pressed === key.letter"
                      on:click.prevent="type(key.letter)">{key.letter}</button>
            {/each}
            {#if r == 2}
              <button class="wide" formaction="?/update" name="key" value="backspace" aria-label="Backspace"
                      class:pressed="pressed === 'backspace'" on:click.prevent="type('backspace')">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6-7z M11 9l6 6 M17 9l-6 6"/></svg>
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</form>
