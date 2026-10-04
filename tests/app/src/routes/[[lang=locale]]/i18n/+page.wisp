---
let n: u32 = cx.query_or("n", 0);
---

<title>{t("i18n.title")}</title>
<h1>{t("i18n.title")}</h1>
<p class="count">{t("i18n.items", n)}</p>
<p class="hi">{t("i18n.hello", name = "<Ann>")}</p>
<p class="locale">{cx.locale()}</p>
<Badge label={t("i18n.items", count = n + 1)} />
<Hello name="Bo" />
<Hello name="Bo" />
<nav>{#each wisp::locales().iter() as l}<a href={wisp::localize(cx.path(), l)}>{l}</a>{/each}</nav>
<button on:click="k++">{:t('i18n.added', k)}</button>
<script>
  let k = 0
</script>
