---
// What app code leaves out: `->` on an action, the check of an email, a
// load for what the markup awaits, `data.` in browser code.
static SENT: Shared<Vec<String>> = Shared::new(Vec::new());

#[action]
fn join(email: Email, n: Option<u8>) {
    if n == Some(0) {
        return error(400, "zero");
    }
    if n == Some(1) {
        return;
    }
    // A closure's `return` is its own.
    let twice: Vec<u8> = [1u8, 2]
        .iter()
        .map(|x| {
            if *x > 5 {
                return 0;
            }
            x * 2
        })
        .collect();
    SENT.lock().push(format!("{email} {}", twice.len()));
    redirect("/t/sugar")
}

async fn count() -> usize {
    SENT.lock().len()
}

let last = SENT.lock().last().cloned();
---
<p id="count">{count().await}</p>
<form action="?/join"><input name="email"></form>
<p id="last" :text="last"></p>
