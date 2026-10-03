//! The pattern matcher behind `check::pattern` (`<input pattern>` as far as
//! it goes): a backtracking one over characters, with a step budget so no
//! input is slow. `wisp-build` reads a rule's pattern with it too, so a
//! pattern that cannot be read stops the build, not a visitor's request.

use std::cell::Cell;

pub enum Node {
    Char(char),
    Any,
    /// Ranges and whether the set is negated.
    Set(Vec<(char, char)>, bool),
    Group(Vec<Vec<Item>>),
}

pub struct Item {
    node: Node,
    min: usize,
    max: usize,
}

pub type Alts = Vec<Vec<Item>>;

pub fn parse(p: &str) -> Option<Alts> {
    let c: Vec<char> = p.chars().collect();
    let mut at = 0;
    let alts = alts(&c, &mut at, 0)?;
    (at == c.len()).then_some(alts)
}

fn alts(c: &[char], at: &mut usize, depth: u32) -> Option<Alts> {
    if depth > 16 {
        return None;
    }
    let mut out = vec![Vec::new()];
    while let Some(&ch) = c.get(*at) {
        *at += 1;
        let node = match ch {
            '|' => {
                out.push(Vec::new());
                continue;
            }
            ')' => {
                *at -= 1;
                break;
            }
            '(' => {
                let g = alts(c, at, depth + 1)?;
                (c.get(*at) == Some(&')')).then_some(())?;
                *at += 1;
                Node::Group(g)
            }
            '.' => Node::Any,
            '[' => set(c, at)?,
            '\\' => escape(*c.get(*at)?, at),
            '?' | '*' | '+' | '{' => return None,
            ch => Node::Char(ch),
        };
        let (min, max) = quantity(c, at)?;
        out.last_mut()?.push(Item { node, min, max });
    }
    Some(out)
}

fn quantity(c: &[char], at: &mut usize) -> Option<(usize, usize)> {
    let q = match c.get(*at) {
        Some('?') => (0, 1),
        Some('*') => (0, usize::MAX),
        Some('+') => (1, usize::MAX),
        Some('{') => {
            let end = c[*at..].iter().position(|&x| x == '}')? + *at;
            let body: String = c[*at + 1..end].iter().collect();
            *at = end;
            match body.split_once(',') {
                None => {
                    let n = body.parse().ok()?;
                    (n, n)
                }
                Some((lo, "")) => (lo.parse().ok()?, usize::MAX),
                Some((lo, hi)) => (lo.parse().ok()?, hi.parse().ok()?),
            }
        }
        _ => return Some((1, 1)),
    };
    *at += 1;
    Some(q)
}

/// `\d \w \s` (capitals negate) or the character itself; `at` moves
/// past the escaped character `ch`.
fn escape(ch: char, at: &mut usize) -> Node {
    *at += 1;
    let ranges: &[(char, char)] = match ch.to_ascii_lowercase() {
        'd' => &[('0', '9')],
        'w' => &[('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')],
        's' => &[(' ', ' '), ('\t', '\r')],
        _ => return Node::Char(ch),
    };
    Node::Set(ranges.to_vec(), ch.is_ascii_uppercase())
}

fn set(c: &[char], at: &mut usize) -> Option<Node> {
    let neg = c.get(*at) == Some(&'^');
    *at += usize::from(neg);
    let mut ranges = Vec::new();
    loop {
        let ch = *c.get(*at)?;
        *at += 1;
        if ch == ']' && !ranges.is_empty() {
            return Some(Node::Set(ranges, neg));
        }
        let lo = if ch == '\\' {
            let e = *c.get(*at)?;
            match escape(e, at) {
                Node::Set(r, false) => {
                    ranges.extend(r);
                    continue;
                }
                // A negated class inside a set is not supported.
                Node::Set(..) => return None,
                _ => e,
            }
        } else {
            ch
        };
        if c.get(*at) == Some(&'-') && c.get(*at + 1).is_some_and(|&x| x != ']') {
            let hi = c[*at + 1];
            *at += 2;
            (lo <= hi).then_some(())?;
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
}

struct Run<'t> {
    text: &'t [char],
    steps: Cell<u32>,
}

pub fn matches(alts: &Alts, s: &str) -> bool {
    let text: Vec<char> = s.chars().collect();
    if text.len() > 1000 {
        return false;
    }
    let end = text.len();
    let run = Run {
        text: &text,
        steps: Cell::new(100_000),
    };
    run.alts(alts, 0, &mut |p| p == end)
}

impl Run<'_> {
    fn alts(&self, alts: &Alts, p: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        alts.iter().any(|seq| self.seq(seq, p, k))
    }

    fn seq(&self, seq: &[Item], p: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        match seq.split_first() {
            None => k(p),
            Some((it, rest)) => self.rep(it, rest, p, 0, k),
        }
    }

    /// `it` as many times as it matches, most first, then `rest`.
    fn rep(
        &self,
        it: &Item,
        rest: &[Item],
        p: usize,
        n: usize,
        k: &mut dyn FnMut(usize) -> bool,
    ) -> bool {
        let left = self.steps.get();
        if left == 0 {
            return false;
        }
        self.steps.set(left - 1);
        // A repeat that took nothing would go on for ever.
        if n < it.max
            && self.one(&it.node, p, &mut |q| {
                (q > p || n < it.min) && self.rep(it, rest, q, n + 1, k)
            })
        {
            return true;
        }
        n >= it.min && self.seq(rest, p, k)
    }

    fn one(&self, node: &Node, p: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if let Node::Group(a) = node {
            return self.alts(a, p, k);
        }
        let Some(&c) = self.text.get(p) else {
            return false;
        };
        let hit = match node {
            Node::Char(x) => *x == c,
            Node::Any => true,
            Node::Set(r, neg) => r.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *neg,
            Node::Group(_) => false,
        };
        hit && k(p + 1)
    }
}
