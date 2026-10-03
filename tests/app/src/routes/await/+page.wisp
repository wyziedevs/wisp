---
async fn slow(ms: u64, n: u32) -> Result<u32> {
    wisp::sleep(std::time::Duration::from_millis(ms)).await;
    Ok(n)
}

async fn refused() -> Result<u32, String> {
    Err("no luck".into())
}

async fn boom() -> u32 {
    panic!("boom")
}

#[action]
fn rename(who: String) {
    let _ = who;
}

let title = "Awaits";
---
<title>{title}</title>
<h1>{title}</h1>
<div id="top"><Tally /></div>
{#await slow(300, 7)}<p id="a">Loading a</p>{:then n}<p id="a">Got {n}</p>{/await}
{#await refused()}<p id="b">Loading b</p>{:then n}<p id="b">{n}</p>{:catch e}<p id="b">Failed: {e}</p>{/await}
<div id="c">{#await boom()}<p>Loading c</p>{/await}</div>
{#await slow(0, 1)}{:then}<p id="d">Quick</p>{/await}
<p id="end">End</p>
{#await slow(100, 5)}<p id="e">Loading e</p>{:then n}<div id="e"><Tally /><Stepper value={n as i32} client:idle /><form action="?/rename"><input aria-label="who" name="who" value="ann"></form></div>{/await}
