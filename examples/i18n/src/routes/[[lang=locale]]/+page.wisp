---
let count: u32 = cx.query_or("count", 0);
let locale = cx.locale();
---

<title>{t("home.title")}</title>
<h1>{t("home.title")}</h1>
<p class="cart">{t("home.cart", count)}</p>
<p class="number">{wisp::format_number(1234567.891, locale)}</p>
<p class="money">{wisp::format_money(1234.5, "EUR", locale)}</p>
<p class="date">{wisp::format_date_long("2026-10-04", locale)}</p>
