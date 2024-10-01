use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::mpsc::{Sender, SyncSender},
};

use chrono::Local;
use log::{error, info};
use serde::Deserialize;

use super::{Filter, Patterns, MFT};
use crate::daemon::MatchManagerInput;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    cmd: Vec<String>,
    filters: BTreeMap<String, Filter>,

    #[serde(skip)]
    name: String,
}

impl Stream {
    pub fn filters(&self) -> &BTreeMap<String, Filter> {
        &self.filters
    }

    pub fn get_filter(&self, filter_name: &str) -> Option<&Filter> {
        self.filters.get(filter_name)
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

        for (key, filter) in &mut self.filters {
            filter.setup(name, key, patterns)?;
        }

        Ok(())
    }

    pub fn manager(
        &'static self,
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
        #[allow(clippy::unwrap_used)]
        // we know there is an stdout because we asked for Stdio::piped()
        let stdout = BufReader::new(child.stdout.take().unwrap());

        // let main handle the child process
        let _ = child_tx.send(Some(child));
        drop(child_tx);

        for line in stdout.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => break,
            };

            for filter in self.filters.values() {
                if let Some(match_) = filter.get_match(&line) {
                    #[allow(clippy::unwrap_used)] // propagating panics is ok
                    match_tx
                        .send(MatchManagerInput::Match(MFT {
                            m: match_,
                            o: filter,
                            t: Local::now(),
                        }))
                        .unwrap();
                }
            }
        }
        error!("stream {} exited", self.name);
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;
    use crate::concepts::filter::tests::ok_filter;

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
        stream.filters.insert("name".into(), ok_filter());
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
