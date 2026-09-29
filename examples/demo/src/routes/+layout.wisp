---
const NAV: [(&str, &str); 3] = [("/", "Home"), ("/about", "About"), ("/wisple", "Wisple")];

// The part of the site this page is in: `/wisple/how-to-play` is Wisple.
let section = cx.path().split('/').nth(1).unwrap_or("");
---
<!-- Wraps every page. Comments like this one never reach the browser. -->
<div class="app">
  <header class="site-header">
    <div class="corner">
      <a href="/" aria-label="Home"><img src="/favicon.svg" alt="" width="32" height="32"></a>
    </div>

    <!-- The wings are drawn in pixels (a 32x48 box for 2rem by 3rem) so their
         outline lands on the same pixel row as the line under the links. -->
    <nav aria-label="Site">
      <svg viewBox="0 0 32 48" aria-hidden="true"><path d="M0,0 L16,32 C24,48 24,48 32,48 L32,0 Z"/><path class="edge" d="M0,0 L16,32 C24,47.5 24,47.5 32,47.5 H33"/></svg>
      <ul>
        {#each NAV as (href, label)}
          <li><a {href} aria-current={(href[1..] == *section).then_some("page")}>{label}</a></li>
        {/each}
      </ul>
      <svg viewBox="0 0 32 48" aria-hidden="true"><path d="M0,0 L0,48 C8,48 8,48 16,32 L32,0 Z"/><path class="edge" d="M-1,47.5 H0 C8,47.5 8,47.5 16,32 L32,0"/></svg>
    </nav>

    <div class="corner">
      <a href="https://github.com/wyziedevs/wisp" aria-label="Wisp on GitHub">
        <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8z"/></svg>
      </a>
    </div>
  </header>

  <main>
    <slot />
  </main>

  <footer>
    <p>© 2026 Wyzie LLC. Open source under the MIT License.</p>
  </footer>
</div>
