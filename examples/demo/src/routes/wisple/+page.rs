// Wisple, a word game in the style of Wordle, and a tour of `load` and form
// actions. The game lives in a cookie, so every visitor has their own, and
// it works with JavaScript turned off: each key on the keyboard is a form
// post. With JavaScript, the page types letters itself and only posts a
// finished guess.

use std::hash::{BuildHasher, RandomState};

const TRIES: usize = 6;
const LEN: usize = 5;
const KEYBOARD: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz";

/// The answers, five lowercase letters each, separated by whitespace.
static WORDS: &str = include_str!("words.txt");

struct Data {
    rows: [Row; TRIES],
    /// The on-screen keyboard, each key marked with the best it has scored.
    keys: [Vec<Tile>; 3],
    /// The row being typed, sent back with the next guess.
    guess: String,
    /// The row is full, so Enter is on and the letters are off.
    full: bool,
    won: bool,
    over: bool,
    answer: &'static str,
    tries: usize,
}

struct Row {
    tiles: [Tile; LEN],
    /// `current` while typing into it, `fresh` for the guess just made.
    class: &'static str,
}

/// A letter on the board or the keyboard.
struct Tile {
    letter: &'static str,
    mark: Mark,
}

/// What a guess says about a letter. Ordered, so a key shows the best it
/// has scored across all guesses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Mark {
    Unknown,
    Missing,
    Close,
    Exact,
}

impl Mark {
    fn class(self) -> &'static str {
        match self {
            Mark::Unknown => "",
            Mark::Missing => "missing",
            Mark::Close => "close",
            Mark::Exact => "exact",
        }
    }

    /// Spoken after the letter by screen readers.
    fn label(self) -> &'static str {
        match self {
            Mark::Unknown => "",
            Mark::Missing => "(not in the word)",
            Mark::Close => "(in the word, wrong place)",
            Mark::Exact => "(correct)",
        }
    }
}

fn load(cx: &mut Cx) -> Data {
    Game::read(cx).data()
}

/// Without JavaScript, every key on the page's keyboard posts here.
#[action]
fn update(cx: &mut Cx, key: String) {
    let mut game = Game::read(cx);
    match key.as_bytes() {
        b"backspace" => {
            game.current.pop();
        }
        &[c] if c.is_ascii_lowercase() && game.current.len() < LEN => game.current.push(c as char),
        _ => {}
    }
    cx.set_cookie("wisple", game);
}

/// A finished guess. It comes from the form, where the page's script typed
/// it; without JavaScript, `update` put the same letters there.
#[action]
fn enter(cx: &mut Cx, guess: String) -> Result<()> {
    let guess = guess.to_ascii_lowercase();
    if guess.len() != LEN || !letters(&guess) {
        return error(400, "A guess is five letters.");
    }
    let mut game = Game::read(cx);
    if !game.over() {
        game.guesses.push_str(&guess);
        game.current.clear();
        cx.set_cookie("wisple", game);
    }
    Ok(())
}

#[action]
fn restart(cx: &mut Cx) {
    cx.set_cookie("wisple", Game::new());
}

// ---- the game ----------------------------------------------------------------

/// A visitor's game, kept in their `wisple` cookie as `42|cranesloth|pi`.
#[derive(Cookie)]
struct Game {
    /// Index into `WORDS`.
    answer: usize,
    /// Every guess so far, run together: `cranesloth` is two.
    guesses: String,
    /// The row being typed.
    current: String,
}

fn words() -> impl Iterator<Item = &'static str> {
    WORDS.split_ascii_whitespace()
}

impl Game {
    fn new() -> Game {
        let answer = RandomState::new().hash_one(0) as usize % words().count();
        Game { answer, guesses: String::new(), current: String::new() }
    }

    /// The visitor's game, or a new one if they have none (or sent us
    /// something that isn't one).
    fn read(cx: &Cx) -> Game {
        let game: Game = cx.cookie_or("wisple", Game::new());
        if game.valid() { game } else { Game::new() }
    }

    /// The cookie comes from the browser, so it is checked like any input.
    fn valid(&self) -> bool {
        self.answer < words().count()
            && self.guesses.len().is_multiple_of(LEN)
            && self.guesses.len() <= LEN * TRIES
            && letters(&self.guesses)
            && self.current.len() <= LEN
            && letters(&self.current)
    }

    fn word(&self) -> &'static str {
        words().nth(self.answer).expect("answer is checked against WORDS")
    }

    fn guesses(&self) -> &[[u8; LEN]] {
        self.guesses.as_bytes().as_chunks().0
    }

    fn won(&self) -> bool {
        self.guesses().last().is_some_and(|g| g == self.word().as_bytes())
    }

    fn over(&self) -> bool {
        self.won() || self.guesses().len() == TRIES
    }

    fn data(&self) -> Data {
        let answer: &[u8; LEN] = self.word().as_bytes().try_into().expect("answers have five letters");
        let guesses = self.guesses();
        let (won, over) = (self.won(), self.over());
        let mut best = [Mark::Unknown; 26];
        let rows = std::array::from_fn(|r| {
            if let Some(guess) = guesses.get(r) {
                let marks = score(guess, answer);
                for (&c, &m) in guess.iter().zip(&marks) {
                    best[index(c)] = best[index(c)].max(m);
                }
                // Fresh until the next row gets its first letter: that is
                // when the page flips the tiles over.
                let fresh = r + 1 == guesses.len() && self.current.is_empty();
                Row {
                    tiles: std::array::from_fn(|i| Tile { letter: letter(guess[i]), mark: marks[i] }),
                    class: if fresh { "fresh" } else { "" },
                }
            } else {
                let current = r == guesses.len() && !over;
                let typed = if current { self.current.as_bytes() } else { b"" };
                Row {
                    tiles: std::array::from_fn(|i| Tile { letter: typed.get(i).map_or("", |&c| letter(c)), mark: Mark::Unknown }),
                    class: if current { "current" } else { "" },
                }
            }
        });
        let keys = KEYBOARD.map(|row| row.bytes().map(|c| Tile { letter: letter(c), mark: best[index(c)] }).collect());
        Data {
            rows,
            keys,
            guess: self.current.clone(),
            full: self.current.len() == LEN,
            won,
            over,
            answer: if over { self.word() } else { "" },
            tries: guesses.len(),
        }
    }
}

/// Wordle's rule: exact letters first. Then each other letter of the guess
/// is close while the answer still has an unclaimed copy of it, so guessing
/// "geese" for "those" marks one `e` close, not three.
fn score(guess: &[u8; LEN], answer: &[u8; LEN]) -> [Mark; LEN] {
    let mut marks = [Mark::Missing; LEN];
    let mut unclaimed = [0u8; 26];
    for i in 0..LEN {
        if guess[i] == answer[i] {
            marks[i] = Mark::Exact;
        } else {
            unclaimed[index(answer[i])] += 1;
        }
    }
    for i in 0..LEN {
        let c = index(guess[i]);
        if marks[i] != Mark::Exact && unclaimed[c] > 0 {
            unclaimed[c] -= 1;
            marks[i] = Mark::Close;
        }
    }
    marks
}

fn letters(s: &str) -> bool {
    s.bytes().all(|c| c.is_ascii_lowercase())
}

fn index(c: u8) -> usize {
    (c - b'a') as usize
}

/// A letter as a `&'static str`, so tiles and keys borrow instead of allocate.
fn letter(c: u8) -> &'static str {
    let i = index(c);
    &ALPHABET[i..i + 1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use Mark::*;

    #[test]
    fn scores_repeated_letters_once() {
        assert_eq!(score(b"geese", b"those"), [Missing, Missing, Missing, Exact, Exact]);
        assert_eq!(score(b"eerie", b"there"), [Close, Missing, Close, Missing, Exact]);
        assert_eq!(score(b"crane", b"crane"), [Exact; LEN]);
    }

    #[test]
    fn words_are_five_letters() {
        assert!(words().all(|w| w.len() == LEN && w.bytes().all(|c| c.is_ascii_lowercase())));
    }

    #[test]
    fn cookie_round_trips_and_rejects_junk() {
        let g: Game = "3|cranesloth|pi".parse().unwrap();
        assert_eq!((g.answer, g.guesses(), g.current.as_str()), (3, &[*b"crane", *b"sloth"][..], "pi"));
        assert_eq!(g.to_string(), "3|cranesloth|pi");
        for junk in ["", "3", "3|cran|", "3|CRANE|", "99999|crane|", "3|crane|toolong", "3|||pi"] {
            assert!(!junk.parse::<Game>().is_ok_and(|g| g.valid()), "{junk}");
        }
    }
}
