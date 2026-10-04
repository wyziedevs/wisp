---
// A realistic page: 50 rows built per request, a name with `<&"` to escape,
// a class chosen by a boolean, and a form. Every server in bench/ sends the
// same content; `bench-run` checks it.
struct Person {
    id: u32,
    name: &'static str,
    score: u32,
    active: bool,
}

const NAMES: [&str; 5] = [
    "Ada <&\"",
    "Alan <&\"",
    "Grace <&\"",
    "Linus <&\"",
    "Edsger <&\"",
];

#[action]
fn default() {}

let people: Vec<Person> = (1..=50)
    .map(|id| Person {
        id,
        name: NAMES[id as usize % 5],
        score: id * 37 % 101,
        active: id % 3 != 0,
    })
    .collect();
---

<title>Roster</title>
<h1>Roster</h1>
<table>
  <thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>
  <tbody>
    {#each people as p}
      <tr class={if p.active { "on" } else { "off" }}><td>{p.id}</td><td>{p.name}</td><td>{p.score}</td></tr>
    {/each}
  </tbody>
</table>
<form method="post"><label>Email <input type="email" name="email" required></label><button>Subscribe</button></form>
