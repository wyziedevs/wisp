//! `wisp build --analyze`: what each route's page sends to the browser,
//! JavaScript, CSS and wasm, raw and gzipped (as Wisp serves it), largest
//! first. Nothing is compiled.

use crate::term;
use std::path::Path;

/// The size of `data` as Wisp gzips it (the runtime's own compressor).
pub fn gzip_size(d: &[u8]) -> usize {
    wisp_shared::gzip::gzip(d).len()
}

/// A route: its pattern, its (raw, gzip) bytes of JS, CSS and wasm, and
/// its files as (name, raw, gzip).
struct Row {
    route: String,
    sums: [(usize, usize); 3],
    files: Vec<(String, usize, usize)>,
}

fn kb(n: usize) -> String {
    format!("{:.1} kB", n as f64 / 1000.0)
}

/// The table: a row per route, its JS, CSS and wasm (raw / gzip), then the
/// files of the heaviest ones.
pub fn run(root: &Path) -> Result<(), String> {
    let mut rows: Vec<Row> = Vec::new();
    for (pattern, files) in wisp_build::analyze(root)? {
        let mut sums = [(0, 0); 3];
        let mut parts = Vec::new();
        for f in &files {
            let k = ["js", "css", "wasm"]
                .iter()
                .position(|&k| k == f.kind)
                .unwrap_or(0);
            let (raw, gz) = (f.bytes.len(), gzip_size(&f.bytes));
            sums[k].0 += raw;
            sums[k].1 += gz;
            parts.push((f.name.clone(), raw, gz));
        }
        rows.push(Row {
            route: pattern,
            sums,
            files: parts,
        });
    }
    if rows.is_empty() {
        println!("The app has no pages yet.");
        return Ok(());
    }
    let total = |r: &Row| r.sums.iter().map(|s| s.1).sum::<usize>();
    rows.sort_by(|a, b| total(b).cmp(&total(a)).then(a.route.cmp(&b.route)));
    term::step("Browser bytes per route, raw / gzip, largest first");
    let w = rows.iter().map(|r| r.route.len()).max().unwrap_or(0).max(5);
    println!(
        "    {}",
        term::dim(&format!(
            "{:w$}  {:>20}  {:>20}  {:>20}",
            "route", "js", "css", "wasm"
        ))
    );
    let cell = |(raw, gz): (usize, usize)| match raw {
        0 => format!("{:>20}", "-"),
        _ => format!("{:>9} /{:>9}", kb(raw), kb(gz)),
    };
    for r in &rows {
        println!(
            "    {:w$}  {}  {}  {}",
            r.route,
            cell(r.sums[0]),
            cell(r.sums[1]),
            cell(r.sums[2])
        );
    }
    println!(
        "\n    {}",
        term::dim(&format!("the files of {}", rows[0].route))
    );
    let mut parts = rows[0].files.clone();
    parts.sort_by_key(|p| std::cmp::Reverse(p.2));
    for (name, raw, gz) in parts {
        println!(
            "    {name}  {}",
            term::dim(&format!("{} / {}", kb(raw), kb(gz)))
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeats_cost_little_and_the_size_is_near_real_gzip() {
        let mut x = 88172645u32;
        let plain: Vec<u8> = (0..4000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect();
        let text = "function draw(a) { return a + 1 }\n"
            .repeat(200)
            .into_bytes();
        assert!(gzip_size(&plain) > 4000 && gzip_size(&plain) < 4000 * 9 / 8 + 40);
        assert!(gzip_size(&text) < 300, "{}", gzip_size(&text));
        assert_eq!(gzip_size(b""), 10 + 2 + 8);
    }
}
