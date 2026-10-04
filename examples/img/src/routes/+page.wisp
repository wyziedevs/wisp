<head>
  <title>Images</title>
</head>

<h1>Images</h1>

<!-- Above the fold: fetchpriority="high", not lazy. -->
<img src="/hero.png" alt="A hero picture" priority>

<!-- Resized on demand by /_img, as next/image does. -->
<img src="/_img?src=/hero.png&w=256&q=75" alt="The same, 256 wide" width="256" height="128">
