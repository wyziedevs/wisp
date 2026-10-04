//! The app's jobs as a host's triggers: `wisp::cron("0 3 * * *", ..)` in
//! `src/` becomes a cron trigger in the host's own config, and an app with
//! `wisp::work` a trigger each minute, which runs the queues' due jobs. The
//! app's code is the same on every host; see `wisp::cron` and the module
//! `wisp::jobs`.

use std::path::Path;

/// What an app asks of the host's cron.
#[derive(Default, Debug, PartialEq)]
pub struct Jobs {
    /// The schedules of `wisp::cron`, each once, in the order they appear.
    pub crons: Vec<String>,
    /// Whether the app has a `wisp::work`.
    pub work: bool,
}

/// The five-field minute trigger of the queues.
const MINUTE: &str = "* * * * *";

impl Jobs {
    /// Whether the app has any.
    pub fn any(&self) -> bool {
        !self.crons.is_empty() || self.work
    }

    /// The schedules the host fires: the app's, and each minute for its queues.
    pub fn triggers(&self) -> Vec<&str> {
        let mut all: Vec<&str> = self.crons.iter().map(String::as_str).collect();
        if self.work && !all.contains(&MINUTE) {
            all.push(MINUTE);
        }
        all
    }
}

/// The path a trigger requests: `/_wisp/cron/<schedule>`.
pub fn path(expr: &str) -> String {
    format!("/_wisp/cron/{}", wisp_shared::cron_slug(expr))
}

/// Reads the app's jobs from its sources. A schedule that is not a string
/// literal cannot be written into the host's config: an error that says where.
pub fn scan(root: &Path) -> Result<Jobs, String> {
    let mut jobs = Jobs::default();
    let mut files = Vec::new();
    rs_files(&root.join("src"), &mut files);
    files.sort();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let shown = file.strip_prefix(root).unwrap_or(&file).display();
        jobs.read(&text, &shown.to_string())?;
    }
    Ok(jobs)
}

fn rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

impl Jobs {
    /// Adds the jobs in `text`, the source of `file`.
    fn read(&mut self, text: &str, file: &str) -> Result<(), String> {
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if call(code, "work").is_some() {
                self.work = true;
            }
            let Some(args) = call(code, "cron") else {
                continue;
            };
            let expr = args
                .trim_start()
                .strip_prefix('"')
                .and_then(|s| s.split_once('"'))
                .map(|(e, _)| e)
                .filter(|e| !e.contains('\\'));
            let Some(expr) = expr else {
                return Err(format!(
                    "{file}:{}: wisp::cron needs its schedule as a string literal here,\nso it can be written into the host's config: cron(\"0 3 * * *\", ..).",
                    n + 1
                ));
            };
            if expr.split_whitespace().count() != 5 {
                return Err(format!(
                    "{file}:{}: the schedule {expr:?} needs five fields: minute hour day month weekday.",
                    n + 1
                ));
            }
            let expr = expr.split_whitespace().collect::<Vec<_>>().join(" ");
            if !self.crons.contains(&expr) {
                self.crons.push(expr);
            }
        }
        Ok(())
    }
}

/// What follows `name(` in `code`, when it is a call of Wisp's: bare (after
/// a `use wisp::name`) or as `wisp::name`, and not a method, a definition
/// or a longer name.
fn call<'a>(code: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!("{name}(");
    let mut from = 0;
    while let Some(i) = code[from..].find(&pat).map(|i| i + from) {
        from = i + pat.len();
        let before = &code[..i];
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let ok = match before.strip_suffix("::") {
            Some(path) => path.ends_with("wisp") && !path.trim_end_matches("wisp").ends_with(ident),
            None => {
                !before.ends_with(ident)
                    && !before.ends_with('.')
                    && !before.trim_end().ends_with("fn")
            }
        };
        if ok {
            return Some(&code[from..]);
        }
    }
    None
}

/// Whether `host` can run an app's jobs, or why not: the build says it
/// before it compiles anything.
pub fn check(host: &str, edge: bool, jobs: &Jobs) -> Result<(), String> {
    if !jobs.any() {
        return Ok(());
    }
    let what = match (jobs.crons.is_empty(), jobs.work) {
        (false, true) => "wisp::cron and wisp::work",
        (false, false) => "wisp::cron",
        _ => "wisp::work",
    };
    let why = match host {
        "cloudflare" | "vercel" => return Ok(()),
        "netlify" if !edge => return Ok(()),
        "netlify" => {
            "Netlify's scheduled functions run beside the app, not in an edge function.\nBuild without --edge"
        }
        "pages" => "Cloudflare Pages has no cron triggers.\nUse --target cloudflare (a Worker)",
        "deno" | "node" | "bun" => {
            "this host's build has no cron trigger of its own to write.\nUse --target cloudflare, vercel or netlify, or run the app's own binary (native or docker), which runs jobs itself"
        }
        "lambda" => {
            "a Lambda is called per request and cannot run jobs between them.\nUse the host's own scheduler (EventBridge) or run the app's own binary (native or docker)"
        }
        _ => return Ok(()),
    };
    Err(format!(
        "The app uses {what}, which the {host} build cannot run: {why}."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Result<Jobs, String> {
        let mut j = Jobs::default();
        j.read(text, "src/hooks.rs").map(|()| j)
    }

    #[test]
    fn the_schedules_and_workers_are_found_in_the_source() {
        let j = read(
            "use wisp::cron;\nasync fn init() {\n    wisp::work(\"mail\", send);\n    wisp::cron(\"0 3 * * *\", || async {});\n    cron( \"*/5  * * * *\", f); // cron(x, y)\n    wisp::cron(\"0 3 * * *\", g);\n}\nfn cron(a: u8) {}\nlet x = y.cron(1);\nlet z = mycron(2);\n",
        )
        .unwrap();
        assert_eq!(j.crons, ["0 3 * * *", "*/5 * * * *"]);
        assert!(j.work);
        assert_eq!(j.triggers(), ["0 3 * * *", "*/5 * * * *", "* * * * *"]);
        let none = read("fn network() { artwork(1); }").unwrap();
        assert_eq!(none, Jobs::default());
    }

    #[test]
    fn a_schedule_that_is_not_a_literal_says_where() {
        let e = read("\n\nwisp::cron(SCHEDULE, f);").unwrap_err();
        assert!(e.starts_with("src/hooks.rs:3: wisp::cron needs"), "{e}");
        assert!(read("wisp::cron(\"0 3 * *\", f);").is_err());
    }

    #[test]
    fn the_minute_trigger_is_not_added_twice() {
        let j = Jobs {
            crons: vec!["* * * * *".into()],
            work: true,
        };
        assert_eq!(j.triggers(), ["* * * * *"]);
        assert_eq!(path("*/5 * * * *"), "/_wisp/cron/*~5_*_*_*_*");
    }

    #[test]
    fn each_host_runs_them_or_says_why_not() {
        let j = Jobs {
            crons: vec!["0 3 * * *".into()],
            work: false,
        };
        for host in ["cloudflare", "vercel", "netlify"] {
            assert!(check(host, false, &j).is_ok(), "{host}");
        }
        assert!(check("vercel", true, &j).is_ok());
        for (host, edge) in [
            ("netlify", true),
            ("pages", false),
            ("deno", false),
            ("node", false),
            ("bun", false),
            ("lambda", false),
        ] {
            let e = check(host, edge, &j).unwrap_err();
            assert!(e.contains("wisp::cron") && e.contains(host), "{e}");
        }
        assert!(check("lambda", false, &Jobs::default()).is_ok());
    }
}
