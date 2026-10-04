//! `wisp build --analyze`: what each route's page sends to the browser,
//! JavaScript, CSS and wasm, raw and gzipped (as Wisp serves it), largest
//! first. Nothing is compiled.

use crate::term;
use std::path::Path;

/// The size of `data` as Wisp gzips it: fixed Huffman codes over a
/// hash-chain matcher (see `compress.rs` in the wisp crate), counted in bits
/// without writing them.
pub fn gzip_size(d: &[u8]) -> usize {
    const WINDOW: usize = 32768;
    const LEN_BASE: [usize; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const DIST_BASE: [usize; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    let hash = |i: usize| {
        let v = u32::from(d[i]) | u32::from(d[i + 1]) << 8 | u32::from(d[i + 2]) << 16;
        (v.wrapping_mul(0x9e37_79b1) >> 17) as usize
    };
    let (mut head, mut prev) = (vec![0u32; 1 << 15], vec![0u32; WINDOW]);
    let mut bits = 3 + 7; // the block header, the end symbol
    let mut i = 0;
    while i < d.len() {
        let (mut best, mut at) = (0, 0);
        if i + 3 <= d.len() {
            let max = (d.len() - i).min(258);
            let mut cand = head[hash(i)] as usize;
            for _ in 0..32 {
                if cand == 0 || i - (cand - 1) > WINDOW {
                    break;
                }
                let p = cand - 1;
                let n = (d[p..p + max].iter().zip(&d[i..i + max]))
                    .take_while(|(a, b)| a == b)
                    .count();
                if n > best {
                    (best, at) = (n, p);
                    if n == max {
                        break;
                    }
                }
                let next = prev[p % WINDOW] as usize;
                if next >= cand {
                    break;
                }
                cand = next;
            }
        }
        let step = if best >= 3 {
            let l = LEN_BASE.partition_point(|&b| b <= best) - 1;
            let c = DIST_BASE.partition_point(|&b| b <= i - at) - 1;
            bits +=
                if l < 23 { 7 } else { 8 } + if l < 8 || l == 28 { 0 } else { (l - 8) / 4 + 1 } + 5;
            bits += if c < 2 { 0 } else { (c - 2) / 2 + 1 };
            best
        } else {
            bits += if d[i] < 144 { 8 } else { 9 };
            1
        };
        for k in (i..i + step).filter(|k| k + 3 <= d.len()) {
            let h = hash(k);
            prev[k % WINDOW] = head[h];
            head[h] = k as u32 + 1;
        }
        i += step;
    }
    10 + bits.div_ceil(8) + 8 // the gzip header and trailer
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
