<title description="A word game in the style of Wordle, written with Wisp">Wisple</title>

<h1 class="visually-hidden">Wisple</h1>

<script>
  let guess = data.guess
  let full = $derived(guess.length === 5)
  let pressed = ''
  let form, grid, enter
  const POP = [{ scale: 1.12 }, { scale: 1 }]
  const SHAKE = { translate: ['0', '-6px', '6px', '-4px', '4px', '0'] }

  watch(() => tries, () => (guess = data.guess))

  function type(key) {
    if (over) return
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
      pressed = key
      setTimeout(() => (pressed = ''), 120)
      type(key)
    }
  }

  function pop(tile) {
    return { update: (letter) => letter && tile.animate(POP, 150) }
  }
</script>

<form class="wisple" action="?/enter" bind:this="form" on:keydown.window="press">
  <a class="how-to-play" href="/wisple/how-to-play" title="How to Play">How to Play</a>

  <div class="grid" class:won={won} bind:this="grid">
    {#each rows as row, r}
      <h2 class="visually-hidden">Row {r + 1}</h2>
      <div class="row {row.class}">
        {#each row.tiles as tile, i}
          <div
            class="letter {tile.mark.class()}"
            style="--i: {i}"
            use:pop="row.class === 'current' && guess[i]">
            <span :text="row.class === 'current' ? guess[i] ?? '' : tile.letter">{tile.letter}</span>
            <span class="visually-hidden">{tile.mark.label()}</span>
          </div>
        {/each}
      </div>
    {/each}
  </div>

  {#if won}
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
      <button
        class="button primary restart"
        formaction="?/restart"
        title="Play Again"
        bind:this="enter">Play Again</button>
    {:else}
      <div class="keyboard">
        {#each keys as row, r}
          <div class="row">
            {#if r == 2}
              <button
                class="wide enter"
                title="Enter the Guess"
                disabled={!full}
                :disabled="!full"
                bind:this="enter">Enter</button>
            {/if}
            {#each row as key}
              <button
                class={key.mark.class()}
                formaction="?/update"
                name="key"
                value={key.letter}
                aria-label="{key.letter} {key.mark.label()}"
                title={key.letter}
                disabled={full}
                :disabled="full"
                class:pressed="pressed === key.letter"
                on:click.prevent="type(key.letter)">{key.letter}</button>
            {/each}
            {#if r == 2}
              <button
                class="wide"
                formaction="?/update"
                name="key"
                value="backspace"
                aria-label="Backspace"
                title="Backspace"
                class:pressed="pressed === 'backspace'"
                on:click.prevent="type('backspace')">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6-7z M11 9l6 6 M17 9l-6 6"/></svg>
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</form>
