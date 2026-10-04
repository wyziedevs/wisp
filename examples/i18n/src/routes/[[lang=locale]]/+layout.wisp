<head>
  {@html wisp::alternates(cx)}
</head>

<header dir={wisp::dir(cx.locale())}>
  <nav>
    <a href={wisp::localize("/", cx.locale())}>{t("nav.home")}</a>
    <a href={wisp::localize("/about", cx.locale())}>{t("nav.about")}</a>
  </nav>
  {@html wisp::switcher(cx)}
</header>

<main>
  <slot />
</main>
