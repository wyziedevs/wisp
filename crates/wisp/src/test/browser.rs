//! Browser tests: the app served on a free port, driven in headless Chrome
//! or Edge over the DevTools protocol (JSON on a WebSocket). No Node, no
//! WebDriver, no dependency.
//!
//! ```ignore
//! #[test]
//! fn counter() {
//!     let mut b = wisp::browser!(App); // returns, skipped, without a browser
//!     b.goto("/");
//!     b.click("text=Plus one");
//!     assert_eq!(b.text("output"), "1");
//! }
//! ```
//!
//! Every action waits for its element (there, visible, enabled, not
//! covered), then for the page to settle: no fetch or navigation under way,
//! loaded, and its DOM still for two frames. So a read after an action sees
//! what the action did. A wait that runs out fails the test with the page's
//! address and the start of its DOM.

use crate::json::{self, Value};
use crate::{App, to_json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// What each page runs before its own scripts: counts fetches under way and
/// DOM changes, notes a navigation starting, and finds elements.
const HELPER: &str = r#"(() => {
  if (window.__wisp) return
  const w = window.__wisp = { busy: 0, leaving: false, changes: 0 }
  const later = () => setTimeout(() => w.busy--)
  const fetch = window.fetch
  window.fetch = function (...args) {
    w.busy++
    return fetch.apply(this, args).then((res) => {
      for (const read of ['text', 'json', 'arrayBuffer', 'blob', 'formData']) {
        res[read] = () => {
          w.busy++
          const body = Response.prototype[read].call(res)
          body.then(later, later)
          return body
        }
      }
      later()
      return res
    }, (e) => { later(); throw e })
  }
  new MutationObserver(() => w.changes++)
    .observe(document, { subtree: true, childList: true, attributes: true, characterData: true })
  const leave = (e) => setTimeout(() => { if (!e.defaultPrevented) w.leaving = true })
  addEventListener('submit', leave)
  addEventListener('click', (e) => {
    const a = e.target.closest && e.target.closest('a[href]')
    if (a && !a.target && !a.hasAttribute('download') && a.href.split('#')[0] !== location.href.split('#')[0]) leave(e)
  })
  const frame = () => new Promise((r) => { requestAnimationFrame(() => setTimeout(r)); setTimeout(r, 100) })
  w.idle = async () => {
    const seen = w.changes
    await frame()
    await frame()
    if (w.busy > 0 || w.leaving || document.readyState !== 'complete') return 0
    return w.changes === seen ? 2 : 1
  }
  const flat = (s) => (s || '').replace(/\s+/g, ' ').trim().toLowerCase()
  w.all = (sel) => {
    if (!sel.startsWith('text=')) return [...document.querySelectorAll(sel)]
    const want = flat(sel.slice(5))
    const found = [...document.querySelectorAll('body *')].filter((el) =>
      !/^(SCRIPT|STYLE|TEMPLATE|NOSCRIPT)$/.test(el.tagName) &&
      [el.innerText, el.getAttribute('aria-label'), el.getAttribute('title')].some((t) => flat(t).includes(want)))
    return found.filter((el) => !found.some((o) => o !== el && el.contains(o)))
  }
  w.run = (sel, act, f) => {
    let all
    try { all = w.all(sel) } catch (e) { return { bad: 'not a selector: ' + e.message } }
    const el = act ? all.find((e) => e.checkVisibility({ visibilityProperty: true }) && !e.disabled) : all[0]
    return el ? f(el) : null
  }
  w.point = (el) => {
    el.scrollIntoView({ block: 'center', inline: 'center', behavior: 'instant' })
    const r = el.getBoundingClientRect(), x = r.x + r.width / 2, y = r.y + r.height / 2
    const hit = document.elementFromPoint(x, y)
    return hit && (hit === el || el.contains(hit)) ? { ok: [x, y] } : null
  }
})()"#;

/// The app on a free port and a headless browser showing it, both stopped
/// when dropped. Made by [`browser`], or [`crate::browser!`] in a test.
pub struct Browser {
    /// `http://127.0.0.1:<port>`, where the app answers.
    base: String,
    ws: Ws,
    /// The page's DevTools session; empty for the browser itself.
    session: String,
    id: i64,
    timeout: Duration,
    /// Dropped after `Browser.close`, it ends the browser for sure.
    _chrome: Process,
    /// Dropped, it stops the app's server.
    _stop: tokio::sync::oneshot::Sender<()>,
}

/// The app served on a free port and a headless Chrome or Edge, or
/// `$WISP_BROWSER`, once [`crate::prepare`] has run. `None`, having said so
/// on stderr, when there is no browser: `wisp::browser!(App)` returns from
/// the test then, so it passes where none is installed.
///
/// Panics if the app or a browser that is there does not start.
pub fn browser<A: App>() -> Option<Browser> {
    let Some(exe) = find() else {
        eprintln!(
            "wisp: no Chrome or Edge found, so this browser test is skipped.\n  Install one, or set WISP_BROWSER to its path."
        );
        return None;
    };
    Some(Browser::start::<A>(&exe).unwrap_or_else(|e| panic!("wisp::test::browser: {e}")))
}

/// `$WISP_BROWSER`, or the first Chrome, Edge, Chromium or Brave in its
/// usual place.
fn find() -> Option<PathBuf> {
    if let Some(exe) = std::env::var_os("WISP_BROWSER") {
        return Some(exe.into());
    }
    let mut places: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Some(dir) = std::env::var_os(var) {
            for exe in [
                r"Google\Chrome\Application\chrome.exe",
                r"Microsoft\Edge\Application\msedge.exe",
                r"Chromium\Application\chrome.exe",
                r"BraveSoftware\Brave-Browser\Application\brave.exe",
            ] {
                places.push(Path::new(&dir).join(exe));
            }
        }
    }
    #[cfg(target_os = "macos")]
    for app in [
        "Google Chrome",
        "Microsoft Edge",
        "Chromium",
        "Brave Browser",
    ] {
        let exe = format!("{app}.app/Contents/MacOS/{app}");
        places.push(Path::new("/Applications").join(&exe));
        if let Some(home) = std::env::var_os("HOME") {
            places.push(Path::new(&home).join("Applications").join(&exe));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    for dir in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for exe in [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "microsoft-edge-stable",
            "brave-browser",
        ] {
            places.push(dir.join(exe));
        }
    }
    places.into_iter().find(|p| p.is_file())
}

impl Browser {
    fn start<A: App>(exe: &Path) -> Result<Browser, String> {
        crate::store::memory();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .map_err(|e| format!("could not listen on a free port: {e}"))?;
        let base = format!(
            "http://{}",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, started) = std::sync::mpsc::channel::<Result<(), String>>();
        std::thread::Builder::new()
            .name("wisp-test-server".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(r) => r,
                    Err(e) => return drop(ready.send(Err(e.to_string()))),
                };
                runtime.block_on(async move {
                    if let Err(e) = crate::prepare::<A>().await {
                        return drop(ready.send(Err(e.to_string())));
                    }
                    let _ = ready.send(Ok(()));
                    let serve = async {
                        if let Err(e) = crate::http::serve_on::<A>(listener).await {
                            eprintln!("wisp::test::browser: the app stopped serving: {e}");
                        }
                    };
                    crate::http::first(serve, async { drop(stopped.await) }).await;
                });
            })
            .map_err(|e| format!("could not start the app's thread: {e}"))?;
        started
            .recv()
            .map_err(|_| "the app's thread ended".to_string())??;

        let mut chrome = Process::launch(exe)?;
        let port = chrome.devtools_port()?;
        let mut b = Browser {
            base,
            ws: Ws::connect(port.0, &port.1)?,
            session: String::new(),
            id: 0,
            timeout: Duration::from_secs(5),
            _chrome: chrome,
            _stop: stop,
        };
        let target = b.call("Target.createTarget", r#"{"url":"about:blank"}"#)?;
        let target = str_of(&target, "targetId")?;
        let session = b.call(
            "Target.attachToTarget",
            &format!(
                r#"{{"targetId":{},"flatten":true}}"#,
                to_json(target.as_str())
            ),
        )?;
        b.session = str_of(&session, "sessionId")?;
        // Scripts for new documents run only with the page domain on.
        b.call("Page.enable", "{}")?;
        b.call(
            "Page.addScriptToEvaluateOnNewDocument",
            &format!(r#"{{"source":{}}}"#, to_json(HELPER)),
        )?;
        b.js(HELPER)?;
        Ok(b)
    }

    /// How long actions and reads wait for their element and the page to
    /// settle before they fail the test: 5 s unless set.
    pub fn timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Loads `path` of the app (or any whole `http…` address) and waits for it.
    pub fn goto(&mut self, path: &str) {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.base)
        };
        // The page now is not idle again: only the new one can be.
        let _ = self.js("window.__wisp && (__wisp.leaving = true)");
        let nav = self.call(
            "Page.navigate",
            &format!(r#"{{"url":{}}}"#, to_json(url.as_str())),
        );
        match nav {
            Ok(v) => match v.get("errorText").and_then(Value::as_str) {
                Some(e) if !e.is_empty() => self.fail(format!("goto({path:?}): {e}")),
                _ => {}
            },
            Err(e) => self.fail(format!("goto({path:?}): {e}")),
        }
        self.settle(&format!("goto({path:?})"));
    }

    /// Clicks the middle of the first visible, enabled element `sel`
    /// matches, as a mouse would.
    pub fn click(&mut self, sel: &str) {
        let what = format!("click({sel:?})");
        let (x, y) = self.point(&what, sel);
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.input(
                &what,
                "Input.dispatchMouseEvent",
                &format!(r#"{{"type":"{kind}","x":{x},"y":{y},"button":"left","clickCount":1}}"#),
            );
        }
        self.settle(&what);
    }

    /// Moves the mouse over the middle of `sel`'s element: `pointermove`,
    /// `mouseover` and `:hover`.
    pub fn hover(&mut self, sel: &str) {
        let what = format!("hover({sel:?})");
        let (x, y) = self.point(&what, sel);
        self.input(
            &what,
            "Input.dispatchMouseEvent",
            &format!(r#"{{"type":"mouseMoved","x":{x},"y":{y}}}"#),
        );
        self.settle(&what);
    }

    /// Replaces the value of `sel`'s input, textarea or editable element
    /// with `text`, as typing would (`input` events).
    pub fn fill(&mut self, sel: &str, text: &str) {
        let what = format!("fill({sel:?})");
        self.wait_for(
            &what,
            sel,
            true,
            "(el) => { el.focus(); if ('value' in el) { el.value = ''; el.dispatchEvent(new Event('input', { bubbles: true })) } else getSelection().selectAllChildren(el); return { ok: true } }",
        );
        if !text.is_empty() {
            self.input(
                &what,
                "Input.insertText",
                &format!(r#"{{"text":{}}}"#, to_json(text)),
            );
        }
        self.settle(&what);
    }

    /// Presses and lets go of one key, on whatever has focus: `"a"`,
    /// `"Enter"`, `"Backspace"`, `"Tab"`, `"Escape"`, `"ArrowLeft"`…
    pub fn press(&mut self, key: &str) {
        let what = format!("press({key:?})");
        let Some((name, code, vk, text)) = key_of(key) else {
            self.fail(format!(
                "{what}: not a key wisp knows; use one character, or a name such as Enter"
            ));
        };
        let text = text.map_or(String::new(), |t| {
            format!(r#","text":{}"#, to_json(t.as_str()))
        });
        let fields = format!(
            r#""key":{},"code":"{code}","windowsVirtualKeyCode":{vk},"nativeVirtualKeyCode":{vk}"#,
            to_json(name.as_str())
        );
        let down = if text.is_empty() {
            "rawKeyDown"
        } else {
            "keyDown"
        };
        self.input(
            &what,
            "Input.dispatchKeyEvent",
            &format!(r#"{{"type":"{down}",{fields}{text}}}"#),
        );
        self.input(
            &what,
            "Input.dispatchKeyEvent",
            &format!(r#"{{"type":"keyUp",{fields}}}"#),
        );
        self.settle(&what);
    }

    /// The text of `sel`'s first element, as shown (`innerText`), trimmed.
    pub fn text(&mut self, sel: &str) -> String {
        let v = self.wait_for(
            &format!("text({sel:?})"),
            sel,
            false,
            "(el) => ({ ok: el.innerText })",
        );
        v.as_str().unwrap_or("").trim().to_string()
    }

    /// An attribute of `sel`'s first element; `None` when it has none.
    pub fn attr(&mut self, sel: &str, name: &str) -> Option<String> {
        let f = format!("(el) => ({{ ok: el.getAttribute({}) }})", to_json(name));
        let v = self.wait_for(&format!("attr({sel:?}, {name:?})"), sel, false, &f);
        v.as_str().map(str::to_string)
    }

    /// How many elements `sel` matches now; no waiting.
    pub fn count(&mut self, sel: &str) -> usize {
        let what = format!("count({sel:?})");
        let expr = format!(
            "(() => {{ try {{ return {{ ok: __wisp.all({}).length }} }} catch (e) {{ return {{ bad: e.message }} }} }})()",
            to_json(sel)
        );
        match self.js(&expr) {
            Ok(v) if v.get("ok").is_some() => {
                v.get("ok").and_then(Value::as_i64).unwrap_or(0) as usize
            }
            Ok(v) => self.fail(format!(
                "{what}: not a selector: {}",
                v.get("bad").and_then(Value::as_str).unwrap_or("")
            )),
            Err(e) => self.fail(format!("{what}: {e}")),
        }
    }

    /// Waits until `sel` matches a visible element.
    pub fn wait(&mut self, sel: &str) {
        self.wait_for(&format!("wait({sel:?})"), sel, true, "() => ({ ok: true })");
    }

    /// Runs JavaScript in the page (a promise is awaited) and gives back
    /// its value as JSON (`Value::Null` for `undefined` or a value that
    /// is not JSON), then waits for the page to settle.
    pub fn eval(&mut self, js: &str) -> Value {
        let what = "eval(..)";
        let v = self
            .js(js)
            .unwrap_or_else(|e| self.fail(format!("{what}: {e}")));
        self.settle(what);
        v
    }

    /// The page's address now.
    pub fn url(&mut self) -> String {
        match self.js("location.href") {
            Ok(Value::String(s)) => s,
            other => self.fail(format!("url(): {other:?}")),
        }
    }

    /// Saves what the page shows (the viewport, 1280×800) as a PNG.
    pub fn screenshot(&mut self, path: impl AsRef<Path>) {
        let path = path.as_ref();
        let what = format!("screenshot({})", path.display());
        let shot = self
            .call("Page.captureScreenshot", r#"{"format":"png"}"#)
            .and_then(|v| str_of(&v, "data"))
            .unwrap_or_else(|e| self.fail(format!("{what}: {e}")));
        let mut png = vec![0u8; shot.len() / 4 * 3 + 3];
        let Some(n) = crate::sign::unbase64(&shot, &mut png) else {
            self.fail(format!("{what}: the browser sent bad base64"));
        };
        png.truncate(n);
        if let Err(e) = std::fs::write(path, &png) {
            self.fail(format!("{what}: {e}"));
        }
    }

    /// The middle of `sel`'s element in the viewport, scrolled into view.
    fn point(&mut self, what: &str, sel: &str) -> (f64, f64) {
        let v = self.wait_for(what, sel, true, "__wisp.point");
        let xy = v.as_array().unwrap_or(&[]);
        match (
            xy.first().and_then(Value::as_f64),
            xy.get(1).and_then(Value::as_f64),
        ) {
            (Some(x), Some(y)) => (x, y),
            _ => self.fail(format!("{what}: no position for the element")),
        }
    }

    /// Runs `f` (a JS function of the element, returning `{ ok: v }` or
    /// null to wait on) on `sel`'s first element (visible and enabled when
    /// `act`) until it gives a value, or the timeout fails the test.
    fn wait_for(&mut self, what: &str, sel: &str, act: bool, f: &str) -> Value {
        let expr = format!("__wisp.run({}, {act}, {f})", to_json(sel));
        let end = Instant::now() + self.timeout;
        let mut last = String::new();
        loop {
            match self.js(&expr) {
                Ok(Value::Null) => {}
                Ok(v) => {
                    if let Some(bad) = v.get("bad").and_then(Value::as_str) {
                        self.fail(format!("{what}: {bad}"));
                    }
                    if let Some(ok) = v.get("ok") {
                        return ok.clone();
                    }
                }
                Err(e) => last = format!(" (last: {e})"),
            }
            if Instant::now() >= end {
                let wanted = if act {
                    "visible, enabled, uncovered element"
                } else {
                    "element"
                };
                self.fail(format!(
                    "{what}: no {wanted} matched in {:?}{last}",
                    self.timeout
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the page to settle: loaded, no fetch or navigation under
    /// way, and no DOM change for two frames (a page that never stops
    /// changing, a clock, counts as settled after half a second).
    fn settle(&mut self, what: &str) {
        let start = Instant::now();
        loop {
            match self.js("window.__wisp ? __wisp.idle() : 0") {
                Ok(v) => match v.as_i64() {
                    Some(2) => return,
                    Some(1) if start.elapsed() > Duration::from_millis(500) => return,
                    _ => {}
                },
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            if start.elapsed() >= self.timeout {
                self.fail(format!(
                    "{what}: the page did not settle in {:?} (a fetch or a navigation still under way)",
                    self.timeout
                ));
            }
        }
    }

    /// A DevTools command to the page that must work.
    fn input(&mut self, what: &str, method: &str, params: &str) {
        if let Err(e) = self.call(method, params) {
            self.fail(format!("{what}: {e}"));
        }
    }

    /// `js`'s value in the page, a promise awaited; `Err` for an exception.
    fn js(&mut self, js: &str) -> Result<Value, String> {
        let r = self.call(
            "Runtime.evaluate",
            &format!(
                r#"{{"expression":{},"returnByValue":true,"awaitPromise":true}}"#,
                to_json(js)
            ),
        )?;
        if let Some(e) = r.get("exceptionDetails") {
            let text = e
                .get("exception")
                .and_then(|x| x.get("description"))
                .or_else(|| e.get("text"))
                .and_then(Value::as_str);
            return Err(text.unwrap_or("an exception").to_string());
        }
        Ok(r.get("result")
            .and_then(|v| v.get("value"))
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// Sends a DevTools command (to the page once attached) and returns its
    /// result, skipping the events that come before it.
    fn call(&mut self, method: &str, params: &str) -> Result<Value, String> {
        self.id += 1;
        let session = if self.session.is_empty() {
            String::new()
        } else {
            format!(r#","sessionId":"{}""#, self.session)
        };
        self.ws.send(&format!(
            r#"{{"id":{},"method":"{method}","params":{params}{session}}}"#,
            self.id
        ))?;
        loop {
            let reply = json::parse(&self.ws.recv()?).map_err(|e| format!("{method}: {e}"))?;
            if reply.get("id").and_then(Value::as_i64) != Some(self.id) {
                continue;
            }
            if let Some(e) = reply.get("error") {
                let msg = e.get("message").and_then(Value::as_str).unwrap_or("failed");
                return Err(format!("{method}: {msg}"));
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Fails the test with `msg`, the page's address and the start of its DOM.
    fn fail(&mut self, msg: String) -> ! {
        let at = match self.js("location.href") {
            Ok(Value::String(s)) => s,
            _ => "?".into(),
        };
        let dom =
            match self.js("(document.body || document.documentElement).outerHTML.slice(0, 1500)") {
                Ok(Value::String(s)) => s,
                _ => "?".into(),
            };
        panic!("wisp browser: {msg}\n  at {at}\n  DOM: {dom}");
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Closed politely, the browser takes its helper processes with it
        // and lets go of its profile; `Process` kills it if not.
        self.session.clear();
        self.ws.quick();
        let _ = self.call("Browser.close", "{}");
    }
}

/// A result's string field.
fn str_of(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("the browser sent no {key}"))
}

/// `key`'s DevTools name, code, virtual key code and text, if it types any.
fn key_of(key: &str) -> Option<(String, String, u32, Option<String>)> {
    let named = |code: &str, vk: u32, text: Option<&str>| {
        Some((
            key.to_string(),
            code.to_string(),
            vk,
            text.map(str::to_string),
        ))
    };
    match key {
        "Enter" => named("Enter", 13, Some("\r")),
        "Tab" => named("Tab", 9, None),
        "Backspace" => named("Backspace", 8, None),
        "Escape" => named("Escape", 27, None),
        "Delete" => named("Delete", 46, None),
        "Home" => named("Home", 36, None),
        "End" => named("End", 35, None),
        "PageUp" => named("PageUp", 33, None),
        "PageDown" => named("PageDown", 34, None),
        "ArrowLeft" => named("ArrowLeft", 37, None),
        "ArrowUp" => named("ArrowUp", 38, None),
        "ArrowRight" => named("ArrowRight", 39, None),
        "ArrowDown" => named("ArrowDown", 40, None),
        " " | "Space" => Some((" ".into(), "Space".into(), 32, Some(" ".into()))),
        _ => {
            let mut chars = key.chars();
            let (c, None) = (chars.next()?, chars.next()) else {
                return None;
            };
            let up = c.to_ascii_uppercase();
            let (code, vk) = match c {
                'a'..='z' | 'A'..='Z' => (format!("Key{up}"), up as u32),
                '0'..='9' => (format!("Digit{c}"), c as u32),
                _ => (String::new(), 0),
            };
            Some((c.to_string(), code, vk, Some(c.to_string())))
        }
    }
}

/// A browser process with a profile of its own, killed (if `Browser.close`
/// did not end it) and its profile removed when dropped. A watchdog kills
/// it should this process die first, when no drop runs.
struct Process {
    child: Child,
    watchdog: Option<Child>,
    profile: PathBuf,
}

impl Process {
    fn launch(exe: &Path) -> Result<Process, String> {
        static N: AtomicU32 = AtomicU32::new(0);
        let profile = std::env::temp_dir().join(format!(
            "wisp-browser-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)
            .map_err(|e| format!("could not make {}: {e}", profile.display()))?;
        let mut cmd = Command::new(exe);
        cmd.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-extensions",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-sync",
            "--disable-breakpad",
            "--mute-audio",
            "--hide-scrollbars",
            "--window-size=1280,800",
        ])
        .arg(format!("--user-data-dir={}", profile.display()));
        // Containers and CI run as root, where Chrome's sandbox will not
        // start; the browser only ever shows the app under test.
        if cfg!(target_os = "linux") {
            cmd.args(["--no-sandbox", "--disable-dev-shm-usage"]);
        }
        let child = cmd
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start {}: {e}", exe.display()))?;
        let watchdog = watchdog(child.id(), &profile);
        Ok(Process {
            child,
            watchdog,
            profile,
        })
    }

    /// The DevTools port and browser path, which the browser writes to its
    /// profile once it listens.
    fn devtools_port(&mut self) -> Result<(u16, String), String> {
        let file = self.profile.join("DevToolsActivePort");
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = std::fs::read_to_string(&file)
                && let mut lines = text.lines()
                && let (Some(Ok(port)), Some(path)) = (lines.next().map(str::parse), lines.next())
                && path.starts_with("/devtools/browser/")
            {
                return Ok((port, path.to_string()));
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                return Err(format!("the browser exited ({status}) before it was ready"));
            }
            if Instant::now() >= end {
                return Err("the browser did not open its DevTools port in 20 s".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let end = Instant::now() + Duration::from_secs(3);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(w) = &mut self.watchdog {
            let _ = w.kill();
            let _ = w.wait();
        }
        // Its helper processes may hold files a moment longer.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A shell that kills `pid` and removes `profile` when its stdin, a pipe
/// this process holds, closes: however this process ends, the system
/// closes it. (`wisp dev` does the same with `wisp __child`, which a test
/// binary does not have.) `None` if no shell starts; the drop still kills
/// the browser.
fn watchdog(pid: u32, profile: &Path) -> Option<Child> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd");
        cmd.raw_arg(format!(
            "/d /c \"findstr x >nul & taskkill /f /t /pid {pid} >nul 2>&1 & rmdir /s /q \"{}\"\"",
            profile.display()
        ));
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        let script = "cat >/dev/null; kill -9 \"$0\" 2>/dev/null; sleep 1; rm -rf \"$1\"";
        cmd.args(["-c", script, &pid.to_string()]).arg(profile);
        cmd
    };
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

/// A WebSocket client (RFC 6455) for the browser's DevTools, blocking, on
/// loopback: text frames out, messages in. `crate::ws` is the server's
/// side, which reads masked frames and writes unmasked ones: the reverse.
struct Ws(BufReader<TcpStream>);

impl Ws {
    fn connect(port: u16, path: &str) -> Result<Ws, String> {
        let lost = |e: std::io::Error| format!("could not reach the browser's DevTools: {e}");
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).map_err(lost)?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(lost)?;
        // The key is any 16 bytes; on loopback the answer needs no check.
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: d2lzcC10ZXN0LWNsaWVudA==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .map_err(lost)?;
        let mut r = BufReader::new(stream);
        let mut line = String::new();
        r.read_line(&mut line).map_err(lost)?;
        if !line.starts_with("HTTP/1.1 101") {
            return Err(format!("the browser's DevTools answered {:?}", line.trim()));
        }
        while line != "\r\n" {
            line.clear();
            if r.read_line(&mut line).map_err(lost)? == 0 {
                return Err("the browser's DevTools closed the connection".into());
            }
        }
        Ok(Ws(r))
    }

    /// A short read timeout, for closing.
    fn quick(&mut self) {
        let _ = self
            .0
            .get_ref()
            .set_read_timeout(Some(Duration::from_secs(2)));
    }

    fn send(&mut self, text: &str) -> Result<(), String> {
        self.frame(1, text.as_bytes())
    }

    /// A final frame from the client, which must be masked: its mask is
    /// zero, so the payload goes as it is.
    fn frame(&mut self, op: u8, payload: &[u8]) -> Result<(), String> {
        let mut f = Vec::with_capacity(payload.len() + 14);
        f.push(0x80 | op);
        match payload.len() {
            n @ 0..126 => f.push(0x80 | n as u8),
            n @ 126..=0xffff => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(&[0; 4]);
        f.extend_from_slice(payload);
        self.0
            .get_mut()
            .write_all(&f)
            .map_err(|e| format!("lost the browser: {e}"))
    }

    /// The next whole message, answering pings on the way.
    fn recv(&mut self) -> Result<String, String> {
        let lost = |e: std::io::Error| format!("lost the browser: {e}");
        let mut msg = Vec::new();
        loop {
            let mut head = [0u8; 2];
            self.0.read_exact(&mut head).map_err(lost)?;
            let (fin, op) = (head[0] & 0x80 != 0, head[0] & 0x0f);
            let len = match head[1] & 0x7f {
                126 => {
                    let mut n = [0u8; 2];
                    self.0.read_exact(&mut n).map_err(lost)?;
                    u16::from_be_bytes(n) as u64
                }
                127 => {
                    let mut n = [0u8; 8];
                    self.0.read_exact(&mut n).map_err(lost)?;
                    u64::from_be_bytes(n)
                }
                n => n as u64,
            };
            if len > 1 << 30 {
                return Err(format!("the browser sent a frame of {len} bytes"));
            }
            let mut mask = [0u8; 4];
            if head[1] & 0x80 != 0 {
                self.0.read_exact(&mut mask).map_err(lost)?;
            }
            let start = msg.len();
            msg.resize(start + len as usize, 0);
            self.0.read_exact(&mut msg[start..]).map_err(lost)?;
            for (i, b) in msg[start..].iter_mut().enumerate() {
                *b ^= mask[i & 3];
            }
            match op {
                0..=2 if fin => {
                    return String::from_utf8(msg)
                        .map_err(|_| "the browser sent text that is not UTF-8".into());
                }
                0..=2 => {}
                8 => return Err("the browser closed the connection".into()),
                9 => {
                    let ping = msg.split_off(start);
                    self.frame(10, &ping)?;
                }
                _ => drop(msg.split_off(start)), // a pong
            }
        }
    }
}
