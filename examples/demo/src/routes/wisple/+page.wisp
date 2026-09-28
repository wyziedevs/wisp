<wisp:head>
  <title>Wisple</title>
  <meta name="description" content="A word game in the style of Wordle, written with Wisp">
</wisp:head>

<h1 class="visually-hidden">Wisple</h1>

<form class="wisple" id="wisple" method="post" action="?/enter">
  <a class="how-to-play" href="/wisple/how-to-play">How to play</a>

  <div class="grid{if data.won { " won" } else { "" }}">
    {#each data.rows as row, r}
      <h2 class="visually-hidden">Row {r + 1}</h2>
      <div class="row {row.class}">
        {#each row.tiles as tile, i}
          <div class="letter {tile.mark.class()}" style="--i: {i}">{tile.letter}<span class="visually-hidden">{tile.mark.label()}</span></div>
        {/each}
      </div>
    {/each}
  </div>

  {#if data.won}
    <!-- A flock of little ghosts rises past the board. -->
    <div class="wisps" aria-hidden="true">
      {#each 0..16 as i}
        <span style="--i: {i}"></span>
      {/each}
    </div>
  {/if}

  <input type="hidden" name="guess" value={data.guess}>

  <div class="controls">
    {#if data.over}
      <p class="result" role="status">
        {#if data.won}
          You got it in {data.tries}!
        {:else}
          The word was <strong>{data.answer}</strong>.
        {/if}
      </p>
      <button class="button primary restart" data-key="enter" formaction="?/restart">Play again</button>
    {:else}
      <div class="keyboard">
        {#each data.keys as row, r}
          <div class="row">
            {#if r == 2}
              <button class="wide enter" data-key="enter"{#if !data.full} disabled{/if}>Enter</button>
            {/if}
            {#each row as key}
              <button class={key.mark.class()} formaction="?/update" name="key" value={key.letter} aria-label="{key.letter} {key.mark.label()}"{#if data.full} disabled{/if}>{key.letter}</button>
            {/each}
            {#if r == 2}
              <button class="wide" formaction="?/update" name="key" value="backspace" aria-label="Backspace">
                <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-6-7z M11 9l6 6 M17 9l-6 6"/></svg>
              </button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
</form>

<script>
  // With JavaScript, letters are typed right here and only a finished guess
  // is posted. Without it, each key is its own form post, which works too.
  (() => {
    const form = document.getElementById('wisple');
    const POP = [{ scale: 1.12 }, { scale: 1 }];
    const SHAKE = { translate: ['0', '-6px', '6px', '-4px', '4px', '0'] };
    const PRESS = [{ translate: '0 2px' }, { translate: '0' }];

    function type(key) {
      const enter = form.querySelector('.enter');
      if (!enter || form.hasAttribute('aria-busy')) return;
      const input = form.elements.guess;
      const guess = key === 'backspace' ? input.value.slice(0, -1) : (input.value + key).slice(0, 5);
      if (guess === input.value) return;
      input.value = guess;
      const tiles = form.querySelectorAll('.current .letter');
      tiles.forEach((tile, i) => (tile.textContent = guess[i] ?? ''));
      if (key !== 'backspace') tiles[guess.length - 1].animate(POP, 150);
      form.querySelectorAll('.keyboard [name=key]:not([value=backspace])').forEach((b) => (b.disabled = guess.length === 5));
      enter.disabled = guess.length < 5;
    }

    form.addEventListener('click', (e) => {
      const key = e.target.closest('[name=key]');
      if (!key) return;
      e.preventDefault();
      type(key.value);
    });

    addEventListener('keydown', (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const key = e.key.toLowerCase();
      if (key === 'enter') {
        e.preventDefault();
        const button = form.querySelector('[data-key=enter]');
        if (!button || form.hasAttribute('aria-busy')) return;
        if (button.disabled) form.querySelector('.current')?.animate(SHAKE, 300); // not five letters yet
        else form.requestSubmit(button);
      } else if (key === 'backspace' || /^[a-z]$/.test(key)) {
        e.preventDefault();
        // The on-screen key dips too, so the two keyboards feel like one.
        form.querySelector(`[name=key][value=${key}]`)?.animate(PRESS, 120);
        type(key);
      }
    });
  })();
</script>
