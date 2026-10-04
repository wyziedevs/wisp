//! Timings, run by hand: `cargo test -r -p wisp-test-app --test speed --
//! --ignored --nocapture`.

#![cfg(not(target_arch = "wasm32"))]

use std::hint::black_box;
use std::time::Instant;
use wisp::App;
use wisp_test_app::Site;

/// The router, on paths as a site gets them: those of routes without
/// parameters (matched whole), then with them, and a miss.
#[test]
#[ignore]
fn routing() {
    let whole = [
        "/",
        "/todos",
        "/a2/nav/one",
        "/sugar/api",
        "/t/readme",
        "/match/opt",
    ];
    let parts = [
        "/match/42",
        "/match/hello-there",
        "/post/some-post",
        "/files/a/b/c.txt",
        "/users/7/items/3",
        "/no/such/page",
    ];
    for (what, paths) in [("whole", whole), ("with parameters", parts)] {
        let rounds = 2_000_000;
        let mut sum = 0;
        let started = Instant::now();
        for _ in 0..rounds {
            for path in paths {
                let routed = Site::route(black_box(path));
                sum += routed.map_or(0, |(id, p)| id + p[0].len());
            }
        }
        let ns = started.elapsed().as_nanos() as f64 / (rounds * paths.len()) as f64;
        println!("routing, {what}: {ns:.1} ns a path ({sum})");
    }
}
