//! Questions on the terminal. Plain lines, not a cursor-driven menu, so they
//! work in every terminal and need nothing but std.

use crate::term::{accent, bold, dim};
use std::io::{self, IsTerminal, Write};

/// Whether a person is there to answer.
pub fn interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn read(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(0) => Err("Cancelled.".into()), // end of input: Ctrl+D, Ctrl+Z
        Ok(_) => Ok(line.trim().to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn prompt(question: &str, hint: &str) -> Result<String, String> {
    read(&format!(
        "{} {} {} ",
        bold(&accent("?")),
        bold(question),
        dim(hint)
    ))
}

pub fn text(question: &str, default: &str) -> Result<String, String> {
    let answer = prompt(question, &format!("({default}) ›"))?;
    Ok(if answer.is_empty() {
        default.to_string()
    } else {
        answer
    })
}

pub fn yes(question: &str, default: bool) -> Result<bool, String> {
    loop {
        match prompt(question, if default { "(Y/n) ›" } else { "(y/N) ›" })?
            .to_ascii_lowercase()
            .as_str()
        {
            "" => return Ok(default),
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => println!("  {}", dim("Answer y or n.")),
        }
    }
}

/// One of `options`, each a (name, description). Returns its index.
pub fn choose(question: &str, options: &[(&str, &str)], default: usize) -> Result<usize, String> {
    assert!(default < options.len());
    println!("{} {}", bold(&accent("?")), bold(question));
    let width = options
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);
    for (i, (name, about)) in options.iter().enumerate() {
        println!(
            "  {}  {}  {}",
            accent(&(i + 1).to_string()),
            bold(&format!("{name:width$}")),
            dim(about)
        );
    }
    loop {
        let answer =
            read(&format!("  {} ", dim(&format!("({}) ›", default + 1))))?.to_ascii_lowercase();
        if answer.is_empty() {
            return Ok(default);
        }
        let by_number = answer
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options.len()).contains(n))
            .map(|n| n - 1);
        let by_name = || {
            options
                .iter()
                .position(|(name, _)| name.to_ascii_lowercase().starts_with(&answer))
        };
        match by_number.or_else(by_name) {
            Some(i) => return Ok(i),
            None => println!("  {}", dim(&format!("Pick 1 to {}.", options.len()))),
        }
    }
}
