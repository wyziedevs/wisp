---
let count: i64 = cx.cookie_or("count", 0);

#[action]
fn increment() {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count + 1);
}

#[action]
fn decrement() {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count - 1);
}
---

<title description="Your new Wisp app">Home</title>

<section class="welcome">
  <div
    class="casper"
    title="Casper"
    aria-hidden="true"
    bind:this="casper"
    class:booing="booing"
    on:pointermove.window="look"
    on:click.window="boo"
    on:animationend="unboo"
    style:--look-x="x"
    style:--look-y="y">
    <span class="boo">boo!</span>
    <svg viewBox="0 0 32 32" id="casper-{count}">
      <path
        class="shape"
        d="M16 2.5C21 2.5 24.5 6.5 24.5 11.5C24.5 13.6 25.3 14.7 26.9 14.7C28.2 14.7 29.3 14 30.2 13.3C31.2 12.6 31.9 13.8 31.3 14.9C30.1 17.4 28.4 19.4 27.1 20.5C26.3 21.2 26.2 22.4 26.5 23.8C26.8 25.2 27.6 26.2 27.6 27.4C27.6 28.6 26.5 29.4 25.4 29.4C24.2 29.4 23.6 27.5 22.3 27.5C21.25 27.5 20.9 28.8 19.8 28.8C18.7 28.8 18.29 27.8 17.2 27.8C15.69 27.8 14.9 29.7 13.6 29.7C12.3 29.7 11.57 27.4 10.1 27.4C8.71 27.4 8.05 29.1 6.8 29.1C5.6 29.1 4.4 28.6 4.4 27.4C4.4 26.2 5.2 25.2 5.5 23.8C5.8 22.4 5.7 21.2 4.9 20.5C3.6 19.4 1.9 17.4 0.7 14.9C0.1 13.8 0.8 12.6 1.8 13.3C2.7 14 3.8 14.7 5.1 14.7C6.7 14.7 7.5 13.6 7.5 11.5C7.5 6.5 11 2.5 16 2.5Z" />
      <ellipse class="eye" cx="12.8" cy="10.8" rx="1.8" ry="2.6" />
      <ellipse class="eye" cx="19.2" cy="10.8" rx="1.8" ry="2.6" />
      <ellipse class="mouth" cx="15.6" cy="16.6" rx="1.3" ry="1.7" />
    </svg>
  </div>

  <h1><span class="hello">Welcome</span> to Your New<br>Wisp App</h1>

  <p class="hint">try editing <code>src/routes/+page.wisp</code></p>

  <form class="counter" method="post">
    <button formaction="?/decrement" aria-label="Decrease the Counter by One" title="Minus One">
      <svg viewBox="0 0 1 1" aria-hidden="true"><path d="M0,0.5 L1,0.5"/></svg>
    </button>
    <output class="count" aria-live="polite" title="Your Count">
      <strong id="count-{count}">{count}</strong>
    </output>
    <button formaction="?/increment" aria-label="Increase the Counter by One" title="Plus One">
      <svg viewBox="0 0 1 1" aria-hidden="true"><path d="M0,0.5 L1,0.5 M0.5,0 L0.5,1"/></svg>
    </button>
  </form>
</section>

<script>
  let casper
  let x = 0, y = 0, booing = false

  function look(e) {
    const box = casper.getBoundingClientRect()
    const dx = e.clientX - (box.left + box.width / 2)
    const dy = e.clientY - (box.top + box.height * 0.35)
    const reach = Math.min(1, Math.hypot(dx, dy) / 240) / (Math.hypot(dx, dy) || 1)
    x = (dx * reach).toFixed(3)
    y = (dy * reach).toFixed(3)
  }

  function boo(e) {
    if (casper.contains(e.target)) booing = true
  }

  function unboo(e) {
    if (e.animationName === 'boo') booing = false
  }
</script>
