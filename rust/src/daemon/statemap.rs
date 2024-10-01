use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use regex::Regex;

use crate::concepts::{ActionFilter, Match, Pattern, Time, MT};

#[derive(Clone)]
pub struct FilterOptions {
    pub stream_name: Option<String>,
    pub filter_name: Option<String>,
    pub patterns: BTreeMap<Arc<Pattern>, Regex>,
}

pub type StateMap<T> = BTreeMap<&'static T, BTreeMap<Match, BTreeSet<Time>>>;

// This trait is needed to permit to implement methods on an external type
pub trait StateMapTrait<T: ActionFilter> {
    fn add(&mut self, mt: &MT<T>);
    fn rm(&mut self, mt: &MT<T>) -> bool;
    fn rm_times(&mut self, mt: &MT<T>) -> Option<BTreeSet<Time>>;
    fn get_times(&self, mt: &MT<T>) -> usize;
    fn filtered(&self, filter_options: FilterOptions) -> Self;
}

impl<T: ActionFilter> StateMapTrait<T> for StateMap<T> {
    fn add(&mut self, mt: &MT<T>) {
        let inner_map = self.entry(mt.o).or_default();
        let inner_set = inner_map.entry(mt.m.clone()).or_default();
        inner_set.insert(mt.t);
    }

    fn rm(&mut self, mt: &MT<T>) -> bool {
        let mut removed = false;
        if let Some(inner_map) = self.get_mut(&mt.o) {
            if let Some(inner_set) = inner_map.get_mut(&mt.m) {
                inner_set.remove(&mt.t);
                removed = true;
                if inner_set.is_empty() {
                    inner_map.remove(&mt.m);
                }
            }
            if inner_map.is_empty() {
                self.remove(&mt.o);
            }
        }
        removed
    }

    fn rm_times(&mut self, mt: &MT<T>) -> Option<BTreeSet<Time>> {
        let mut set = None;
        if let Some(inner_map) = self.get_mut(&mt.o) {
            set = inner_map.remove(&mt.m);
            if inner_map.is_empty() {
                self.remove(&mt.o);
            }
        }
        set
    }

    fn get_times(&self, mt: &MT<T>) -> usize {
        match self.get(&mt.o).and_then(|map| map.get(&mt.m)) {
            Some(x) => x.len(),
            None => 0,
        }
    }

    fn filtered(&self, filter_options: FilterOptions) -> Self {
        let FilterOptions {
            stream_name,
            filter_name,
            patterns,
        } = filter_options;
        self.iter()
            // stream/filter filtering
            .filter(|(object, _)| {
                if let Some(stream_name) = &stream_name {
                    let full_name = object.full_name();
                    let (s, f) = (full_name.0, full_name.1);
                    if *stream_name != s {
                        return false;
                    }
                    if let Some(filter_name) = &filter_name {
                        if *filter_name != f {
                            return false;
                        }
                    }
                }
                return true;
            })
            // pattern filtering
            .filter(|(object, _)| {
                patterns
                    .iter()
                    .all(|(pattern, _)| object.patterns().get(pattern).is_some())
            })
            // match filtering
            .filter_map(|(object, inner_map)| {
                let map: BTreeMap<Match, BTreeSet<Time>> = inner_map
                    .iter()
                    .filter(|(match_, _)| {
                        match_
                            .into_iter()
                            .zip(object.patterns())
                            .filter_map(|(a_match, pattern)| match patterns.get(pattern.as_ref()) {
                                Some(regex) => Some((a_match, regex)),
                                None => None,
                            })
                            .all(|(a_match, regex)| regex.is_match(a_match))
                    })
                    .map(|(a, b)| (a.clone(), b.clone()))
                    .collect();
                if map.len() > 0 {
                    Some((*object, map))
                } else {
                    None
                }
            })
            .collect()
    }
}
