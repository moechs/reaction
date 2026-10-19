use std::{
    collections::BTreeMap,
    error::Error,
    io::{BufRead, BufReader, stdin},
    path::PathBuf,
};

use crate::concepts::{Action, Config, Filter, FilterMethodRegex, LineContext};

pub fn test_regex(
    config_path: Option<PathBuf>,
    regex: String,
    line: Option<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config = Config::from_path(&config_path)?;

    let mut filter = Filter {
        option_regex: Some(FilterMethodRegex {
            regex: vec![regex],
            ..Default::default()
        }),
        actions: BTreeMap::from([(
            "dummy".into(),
            Action {
                cmd: vec!["echo".into(), "dummy".into()],
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    filter.setup("dummy_stream", "dummy_filter", &config.patterns)?;

    match_filter_on_line_s(&filter, line);
    Ok(())
}

pub fn test_filter(
    config_path: Option<PathBuf>,
    stream_filter: String,
    line: Option<String>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let config = Config::from_path(&config_path)?;
    let (stream_name, filter_name)=
        stream_filter.split_once(".").ok_or_else(|| format!("the STREAM.FILTER must contain a `.` to delimit the stream from the filter: {stream_filter}"))?;

    let stream = config.streams.get(stream_name).ok_or(format!(
        "No stream {stream_name} present in the configuration. Available streams: {:?}",
        config.streams.keys()
    ))?;

    let filter = stream.filters.get(filter_name).ok_or(format!(
        "No filter {filter_name} present in stream {stream_name}. Available filters: {:?}",
        stream.filters.keys()
    ))?;

    match_filter_on_line_s(filter, line);
    Ok(())
}

fn match_filter_on_line_s(filter: &Filter, line: Option<String>) {
    let match_closure = |line: String| {
        match filter.get_match(&line, &mut LineContext::default(), true) {
            Ok(Some(match_)) => {
                println!("\x1b[32mmatching\x1b[0m {match_:?}: {line}");
            }
            Ok(None) => match filter.get_match(&line, &mut LineContext::default(), false) {
                Ok(Some(match_)) => {
                    println!("\x1b[33mignore matching\x1b[0m {match_:?}: {line}");
                }
                Ok(None) => {
                    println!("\x1b[31mno match\x1b[0m: {line}");
                }
                Err(_) => {}
            },
            Err(err) => {
                println!("\x1b[31merror while trying to match ({err})\x1b[0m: {line}");
            }
        };
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
}
