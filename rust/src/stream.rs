use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{Sender, SyncSender},
        Arc,
    },
};

use chrono::Local;
use log::{error, info};
use serde::Deserialize;

use crate::{config::Patterns, filter::Filter, matches::MatchManagerInput, messages::MFT};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    cmd: Vec<String>,
    filters: BTreeMap<String, Arc<Filter>>,

    #[serde(skip)]
    name: String,
}

impl Stream {
    pub fn filters(&self) -> &BTreeMap<String, Arc<Filter>> {
        &self.filters
    }

    pub fn setup(&mut self, name: &str, patterns: &Patterns) -> Result<(), String> {
        self._setup(name, patterns)
            .map_err(|msg| format!("stream {}: {}", name, msg))
    }

    fn _setup(&mut self, name: &str, patterns: &Patterns) -> Result<(), String> {
        self.name = name.to_string();

        if self.name.is_empty() {
            return Err("stream name is empty".into());
        }
        if self.name.contains('.') {
            return Err("character '.' is not allowed in stream name".into());
        }

        if self.cmd.is_empty() {
            return Err("cmd is empty".into());
        }
        if self.cmd[0].is_empty() {
            return Err("cmd's first item is empty".into());
        }

        if self.filters.is_empty() {
            return Err("no filters configured".into());
        }

        let mut new_filters = BTreeMap::new();
        for (key, filter) in &self.filters {
            let mut new_filter = filter.as_ref().clone();
            new_filter.setup(name, key, patterns)?;
            new_filters.insert(key.clone(), Arc::new(new_filter));
        }
        self.filters = new_filters;

        Ok(())
    }

    pub fn manager(
        &self,
        child_tx: SyncSender<Option<Child>>,
        match_tx: Sender<MatchManagerInput>,
    ) {
        info!("{}: start {:?}", self.name, self.cmd);
        let mut child = match Command::new(&self.cmd[0])
            .args(&self.cmd[1..])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                error!("could not execute stream {} cmd: {}", self.name, err);
                let _ = child_tx.send(None);
                return;
            }
        };

        // keep stdout before sending/moving child to the main thread
        let stdout = BufReader::new(child.stdout.take().unwrap());

        // let main handle the child process
        let _ = child_tx.send(Some(child));
        drop(child_tx);

        // let mut line: String = "".into();
        // while let Ok(nb_chars) = stdout.read_line(&mut line) {
        for line in stdout.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };
            // if nb_chars == 0 {
            //     break;
            // }
            // line.pop(); // remove trailing newline

            for filter in self.filters.values() {
                if let Some(match_) = filter.get_match(&line) {
                    match_tx
                        .send(MatchManagerInput::Match(MFT {
                            m: match_,
                            f: filter.clone(),
                            t: Local::now(),
                        }))
                        .unwrap();
                }
            }

            // line.clear();
        }
        info!("stream {} exited", self.name);
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;

    fn default_stream() -> Stream {
        Stream {
            cmd: Vec::new(),
            name: "".into(),
            filters: BTreeMap::new(),
        }
    }

    pub fn ok_stream() -> Stream {
        let mut stream = default_stream();
        stream.cmd = vec!["command".into()];
        stream
            .filters
            .insert("name".into(), Arc::new(crate::filter::tests::ok_filter()));
        stream
    }

    #[test]
    fn test() {
        let mut stream;
        let name = "name";

        // missing cmd
        stream = ok_stream();
        stream.cmd = Vec::new();
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // missing cmd
        stream = ok_stream();
        stream.cmd = vec!["".into(), "arg1".into()];
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // missing filters
        stream = ok_stream();
        stream.filters = BTreeMap::new();
        assert!(stream.setup(name, &BTreeMap::new()).is_err());

        // stream ok
        stream = ok_stream();
        assert!(stream.setup(name, &BTreeMap::new()).is_ok());
    }
}
