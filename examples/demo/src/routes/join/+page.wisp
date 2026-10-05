---
// A form that keeps what was typed: send it with something missing and
// every problem shows beside its field (the server checks; the browser
// checks what it can first), each choice staying as made. With
// JavaScript on the page morphs in place; off, it loads. Same markup.
#[action]
fn join(
    #[validate(len = 2..=40)] name: String,
    plan: String,
    #[validate(min_len = 1)] days: Vec<String>,
    news: bool,
    terms: bool,
) {
    if !terms {
        return invalid("terms", "Tick to accept the terms");
    }
    let _ = (name, plan, days, news);
    cx.flash("Welcome aboard");
    redirect("/join")
}

let plan = "free";
---

<title description="A form that keeps what you typed">Join</title>

<div class="text-column">
  <h1>Join</h1>
  <p>Leave the name short, or the terms unticked, and press Join: each problem shows by its field and your choices stay. Try it with JavaScript off too.</p>
  {@flash}
  <form action="?/join" class="join">
    <label for="name">Name</label>
    <input id="name" name="name">
    <label for="plan">Plan</label>
    <select id="plan" name="plan" value={plan}>
      <option value="free">Free</option>
      <option value="pro">Pro</option>
    </select>
    <fieldset>
      <legend>Days</legend>
      {#each ["Mon", "Wed", "Fri"] as day}
        <label><input type="checkbox" name="days" value={day}> {day}</label>
      {/each}
    </fieldset>
    <label><input type="checkbox" name="news" checked> Send me the newsletter</label>
    <label><input type="checkbox" name="terms" id="terms"> I accept the terms</label>
    <button class="button">Join</button>
  </form>
</div>
