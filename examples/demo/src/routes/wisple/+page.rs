//! Wisple, a word game in the style of Wordle, and a tour of `load` and form
//! actions. The game lives in a cookie, so every visitor has their own, and
//! it works with JavaScript turned off: each key on the keyboard is a form
//! post. With JavaScript, the page types letters itself and only posts a
//! finished guess.

use std::hash::{BuildHasher, RandomState};
use wisp::prelude::*;

const TRIES: usize = 6;
const LEN: usize = 5;
const KEYBOARD: [&str; 3] = ["qwertyuiop", "asdfghjkl", "zxcvbnm"];
const ALPHABET: &str = "abcdefghijklmnopqrstuvwxyz";

/// The answers, five lowercase letters each, separated by whitespace.
static WORDS: &str = include_str!("words.txt");

pub struct Data {
    pub rows: [Row; TRIES],
    pub keys: [Vec<Key>; 3],
    /// The row being typed, sent back with the next guess.
    pub guess: String,
    /// The row is full, so Enter is on and the letters are off.
    pub full: bool,
    pub won: bool,
    pub over: bool,
    pub answer: &'static str,
    pub tries: usize,
}

pub struct Row {
    pub tiles: [Tile; LEN],
    /// `current` while typing into it, `fresh` for the guess just made.
    pub class: &'static str,
}

pub struct Tile {
    pub letter: &'static str,
    pub mark: Mark,
}

pub struct Key {
    pub letter: &'static str,
    pub mark: Mark,
}

/// What a guess says about a letter. Ordered, so a key shows the best it
/// has scored across all guesses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Mark {
    Unknown,
    Missing,
    Close,
    Exact,
}

impl Mark {
    pub fn class(self) -> &'static str {
        match self {
            Mark::Unknown => "",
            Mark::Missing => "missing",
            Mark::Close => "close",
            Mark::Exact => "exact",
        }
    }

    /// Spoken after the letter by screen readers.
    pub fn label(self) -> &'static str {
        match self {
            Mark::Unknown => "",
            Mark::Missing => "(not in the word)",
            Mark::Close => "(in the word, wrong place)",
            Mark::Exact => "(correct)",
        }
    }
}

pub async fn load(cx: &mut Cx) -> Result<Data> {
    Ok(Game::read(cx).data())
}

/// Without JavaScript, every key on the page's keyboard posts here.
#[action]
pub async fn update(cx: &mut Cx) -> Result<()> {
    let mut game = Game::read(cx);
    let key = cx.form().required("key")?;
    match key.as_bytes() {
        b"backspace" => {
            game.current.pop();
        }
        &[c] if c.is_ascii_lowercase() && game.current.len() < LEN => game.current.push(c as char),
        _ => {}
    }
    game.save(cx);
    Ok(())
}

/// A finished guess. It comes from the form, where the page's script typed
/// it; without JavaScript, `update` put the same letters there.
#[action]
pub async fn enter(cx: &mut Cx) -> Result<()> {
    let mut game = Game::read(cx);
    let guess = cx.form().required("guess")?.to_ascii_lowercase();
    let guess: [u8; LEN] = guess.as_bytes().try_into().map_err(|_| error(400, "a guess has five letters"))?;
    if !guess.iter().all(u8::is_ascii_lowercase) {
        return Err(error(400, "a guess is letters only"));
    }
    if !game.over() {
        game.guesses.push(guess);
        game.current.clear();
        game.save(cx);
    }
    Ok(())
}

#[action]
pub async fn restart(cx: &mut Cx) -> Result<()> {
    Game::new().save(cx);
    Ok(())
}

// ---- the game ----------------------------------------------------------------

struct Game {
    /// Index into `WORDS`.
    answer: usize,
    guesses: Vec<[u8; LEN]>,
    current: String,
}

fn words() -> impl Iterator<Item = &'static str> {
    WORDS.split_ascii_whitespace()
}

impl Game {
    fn new() -> Game {
        let answer = RandomState::new().hash_one(0) as usize % words().count();
        Game { answer, guesses: Vec::new(), current: String::new() }
    }

    /// The visitor's game, or a new one if they have none (or sent us
    /// something that isn't one).
    fn read(cx: &mut Cx) -> Game {
        match cx.cookie("wisple").and_then(Game::parse) {
            Some(game) => game,
            None => {
                let game = Game::new();
                game.save(cx);
                game
            }
        }
    }

    /// The cookie is `answer-guesses-current`, like `42-cranesloth-pi`.
    fn parse(s: &str) -> Option<Game> {
        let mut parts = s.split('-');
        let answer: usize = parts.next()?.parse().ok()?;
        let (guesses, current) = (parts.next()?.as_bytes(), parts.next()?);
        let letters = |s: &[u8]| s.iter().all(u8::is_ascii_lowercase);
        let valid = answer < words().count()
            && guesses.len() % LEN == 0
            && guesses.len() <= LEN * TRIES
            && letters(guesses)
            && current.len() <= LEN
            && letters(current.as_bytes())
            && parts.next().is_none();
        valid.then(|| Game {
            answer,
            guesses: guesses.chunks_exact(LEN).map(|g| g.try_into().unwrap()).collect(),
            current: current.to_string(),
        })
    }

    fn save(&self, cx: &mut Cx) {
        let guesses: Vec<u8> = self.guesses.concat();
        let guesses = std::str::from_utf8(&guesses).expect("guesses are ASCII");
        cx.set_cookie("wisple", &format!("{}-{guesses}-{}", self.answer, self.current));
    }

    fn word(&self) -> &'static str {
        words().nth(self.answer).expect("answer is checked against WORDS")
    }

    fn won(&self) -> bool {
        self.guesses.last().is_some_and(|g| g == self.word().as_bytes())
    }

    fn over(&self) -> bool {
        self.won() || self.guesses.len() == TRIES
    }

    fn data(&self) -> Data {
        let answer: [u8; LEN] = self.word().as_bytes().try_into().expect("answers have five letters");
        let (won, over) = (self.won(), self.over());
        let mut best = [Mark::Unknown; 26];
        let rows = std::array::from_fn(|r| {
            if let Some(guess) = self.guesses.get(r) {
                let marks = score(guess, &answer);
                for (&c, &m) in guess.iter().zip(&marks) {
                    best[index(c)] = best[index(c)].max(m);
                }
                // Fresh until the next row gets its first letter: that is
                // when the page flips the tiles over.
                let fresh = r + 1 == self.guesses.len() && self.current.is_empty();
                Row {
                    tiles: std::array::from_fn(|i| Tile { letter: letter(guess[i]), mark: marks[i] }),
                    class: if fresh { "fresh" } else { "" },
                }
            } else {
                let current = r == self.guesses.len() && !over;
                let typed = if current { self.current.as_bytes() } else { b"" };
                Row {
                    tiles: std::array::from_fn(|i| Tile { letter: typed.get(i).map_or("", |&c| letter(c)), mark: Mark::Unknown }),
                    class: if current { "current" } else { "" },
                }
            }
        });
        let keys = KEYBOARD.map(|row| row.bytes().map(|c| Key { letter: letter(c), mark: best[index(c)] }).collect());
        Data {
            rows,
            keys,
            guess: self.current.clone(),
            full: self.current.len() == LEN,
            won,
            over,
            answer: if over { self.word() } else { "" },
            tries: self.guesses.len(),
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
        let g = Game::parse("3-cranesloth-pi").unwrap();
        assert_eq!((g.answer, g.guesses.len(), g.current.as_str()), (3, 2, "pi"));
        for junk in ["", "3", "3-cran-", "3-CRANE-", "99999-crane-", "3-crane-toolong", "3--pi-x"] {
            assert!(Game::parse(junk).is_none(), "{junk}");
        }
    }
}
