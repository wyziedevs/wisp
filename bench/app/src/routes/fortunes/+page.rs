// TechEmpower's "fortunes", minus the database: copy the rows, add one,
// sort by message, render with escaping. `bench/aspnet` does the same.

struct Fortune {
    id: u32,
    message: &'static str,
}

struct Data {
    fortunes: Vec<Fortune>,
}

const ROWS: [(u32, &str); 12] = [
    (1, "fortune: No such file or directory"),
    (2, "A computer scientist is someone who fixes things that aren't broken."),
    (3, "After enough decimal places, nobody gives a damn."),
    (4, "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"),
    (5, "A computer program does what you tell it to do, not what you want it to do."),
    (6, "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"),
    (7, "Any program that runs right is obsolete."),
    (8, "A list is only as strong as its weakest link. — Donald Knuth"),
    (9, "Feature: A bug with seniority."),
    (10, "Computers make very fast, very accurate mistakes."),
    (11, "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"),
    (12, "フレームワークのベンチマーク"),
];

fn load() -> Data {
    let mut fortunes = Vec::with_capacity(ROWS.len() + 1);
    fortunes.extend(ROWS.iter().map(|&(id, message)| Fortune { id, message }));
    fortunes.push(Fortune { id: 0, message: "Additional fortune added at request time." });
    fortunes.sort_unstable_by(|a, b| a.message.cmp(b.message));
    Data { fortunes }
}
