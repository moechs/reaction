use std::{
    collections::BTreeSet,
    error::Error,
    io::{stdin, BufRead, BufReader},
    path::PathBuf,
    sync::Arc,
};

use regex::Regex;

use crate::concepts::{Config, Pattern};

pub fn test_regex(
    config_path: PathBuf,
    mut regex: String,
    line: Option<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config: Config = Config::from_file(&config_path)?;

    // Code close to Filter::setup()
    let mut used_patterns: BTreeSet<Arc<Pattern>> = BTreeSet::new();
    for pattern in config.patterns().values() {
        if let Some(index) = regex.find(pattern.name_with_braces()) {
            // we already `find` it, so we must be able to `rfind` it
            #[allow(clippy::unwrap_used)]
            if regex.rfind(pattern.name_with_braces()).unwrap() != index {
                return Err(format!(
                    "pattern {} present multiple times in regex",
                    pattern.name_with_braces()
                )
                .into());
            }
            used_patterns.insert(pattern.clone());
        }
        regex = regex.replacen(pattern.name_with_braces(), &pattern.regex, 1);
    }

    let compiled = Regex::new(&regex).map_err(|err| format!("regex doesn't compile: {err}"))?;

    let match_closure = |line: String| {
        let mut ignored = false;
        if let Some(matches) = compiled.captures(&line) {
            let mut result = Vec::new();
            if !used_patterns.is_empty() {
                for pattern in used_patterns.iter() {
                    if let Some(match_) = matches.name(pattern.name()) {
                        result.push(match_.as_str().to_string());
                        if !pattern.not_an_ignore(match_.as_str()) {
                            ignored = true;
                        }
                    }
                }
                if !ignored {
                    println!("\x1b[32mmatching\x1b[0m {result:?}: {line}");
                } else {
                    println!("\x1b[33mignore matching\x1b[0m {result:?}: {line}");
                }
            } else {
                println!("\x1b[32mmatching\x1b[0m: {line}");
            }
        } else {
            println!("\x1b[31mno match\x1b[0m: {line}");
        }
    };

    if let Some(line) = line {
        match_closure(line);
    } else {
        eprintln!("no second argument: reading from stdin");
        for line in BufReader::new(stdin()).lines() {
            match line {
                Ok(line) => match_closure(line),
                Err(_) => break,
            };
        }
    }
    Ok(())
}
