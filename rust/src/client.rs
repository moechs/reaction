use std::path::PathBuf;

use regex::Regex;

use crate::cli::{Format, NamedRegex};

pub fn show(
    socket: &PathBuf,
    format: Format,
    limit: &Option<String>,
    pattern: &Option<Regex>,
    patterns: &Vec<NamedRegex>,
) {
    println!("show {:?} {:?} {:?} {:?} {:?}", socket, format, limit, pattern, patterns);
}

pub fn flush(
    socket: &PathBuf,
    format: Format,
    limit: &Option<String>,
    pattern: &Option<Regex>,
    patterns: &Vec<NamedRegex>,
) {
    println!("flush {:?} {:?} {:?} {:?} {:?}", socket, format, limit, pattern, patterns);
}

pub fn test_regex(config_path: &PathBuf, regex: &String, line: &Option<String>) {
    println!("test-regex {:?} {:?} {:?} ", config_path, regex, line);
}
