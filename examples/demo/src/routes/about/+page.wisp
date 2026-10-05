<head>
  <title>About</title>
  <meta name="description" content="About Wisp">
</head>

<div class="text-column">
  <h1>About Wisp</h1>

  <p>This is a Wisp app: pages written in <code>.wisp</code> templates, logic in plain Rust, all compiled into one small binary. To make your own, first install the <code>wisp</code> command from <a href="https://github.com/wyziedevs/wisp" title="Wisp on GitHub">GitHub</a>. You need <a href="https://rustup.rs" title="Install Rust">Rust</a>:</p>

  <pre><code>cargo install wisp-web</code></pre>

  <p>Then create an app anywhere, answer a few questions, and start it:</p>

  <pre><code>wisp new my-app
cd my-app
wisp dev</code></pre>

  <p>The page you're looking at is plain HTML, rendered on the server. It has no <code>+page.rs</code>, so there is no data to load, and it needs no JavaScript at all. Try viewing the page's source.</p>

  <p>The <a href="/wisple" title="Play Wisple">Wisple</a> page shows off loading data and handling forms. Try playing it with JavaScript turned off!</p>
</div>
