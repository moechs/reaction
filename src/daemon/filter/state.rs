use std::collections::{BTreeMap, BTreeSet};

use crate::{
    concepts::{Filter, Match, MatchTime, Time},
    treedb::{
        helpers::{to_match, to_matchtime, to_time, to_timemap, to_u64},
        Database, Tree,
    },
};

pub fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name,
        filter.name
    )
}

pub fn filter_triggers_old_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name, filter.name)
}

pub fn filter_triggers_db_name(filter: &Filter) -> String {
    format!(
        "filter_triggers2_{}.{}",
        filter.stream_name,
        filter.name
    )
}

/// Internal state of a [`FilterManager`].
/// Holds all data on current matches and triggers.
pub struct State {
    /// the Filter managed
    filter: &'static Filter,
    /// Has the filter at least an action with an after directive?
    has_after: bool,
    /// Saves all the current Matches for this Filter
    /// Has duplicate values for a key
    /// Not persisted
    pub matches: BTreeMap<Match, BTreeSet<Time>>,
    /// Alternative view of the current Matches for O(1) cleaning of old Matches
    /// without added async Tasks to remove them
    /// Persisted
    ///
    /// I'm pretty confident that Time will always be unique, because it has enough precision.
    /// See this code that gives different times, even in a minimal loop:
    /// ```rust
    /// use chrono::{Local};
    ///
    /// let mut res = vec![];
    /// for _ in 0..10 {
    ///     let now = Local::now();
    ///     res.push(format!("Now: {now}"));
    /// }
    /// for s in res {
    ///     println!("{s}");
    /// }
    /// ```
    pub ordered_times: Tree<Time, Match>,
    /// Saves all the current Triggers for this Filter
    /// Persisted
    pub triggers: Tree<Match, BTreeMap<Time, u64>>,
}

impl State {
    pub fn new(filter: &'static Filter, db: &mut Database, now: Time) -> Result<Self, String> {
        let ordered_times = db.open_tree(
            filter_ordered_times_db_name(filter),
            filter.retry_duration.unwrap_or_default(),
            |(key, value)| Ok((to_time(&key)?, to_match(&value)?)),
        )?;
        let mut triggers = db.open_tree(
            filter_triggers_db_name(filter),
            filter.longuest_action_duration,
            |(key, value)| Ok((to_match(&key)?, to_timemap(&value)?)),
        )?;
        if triggers.is_empty() {
            let old_triggers = db.open_tree(
                filter_triggers_old_db_name(filter),
                filter.longuest_action_duration,
                |(key, value)| Ok((to_matchtime(&key)?, to_u64(&value)?)),
            )?;
            for (mt, n) in old_triggers.iter() {
                triggers.fetch_update(mt.m.clone(), |map| {
                    Some(match map {
                        None => [(mt.t, *n)].into(),
                        Some(mut map) => {
                            map.insert(mt.t, *n);
                            map
                        }
                    })
                });
            }
        }
        let mut this = Self {
            filter,
            has_after: !filter.longuest_action_duration.is_zero(),
            matches: BTreeMap::new(),
            ordered_times,
            triggers,
        };
        this.clear_past_matches(now);
        this.load_matches_from_ordered_times();
        Ok(this)
    }

    pub fn add_match(&mut self, m: Match, t: Time) {
        let set = self.matches.entry(m.clone()).or_default();
        set.insert(t);
        self.ordered_times.insert(t, m);
    }

    pub fn add_trigger(&mut self, m: Match, t: Time) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            let n = self.filter.actions.len() as u64;
            self.triggers.fetch_update(m, |map| {
                Some(match map {
                    None => [(t, n)].into(),
                    Some(mut value) => {
                        value.insert(t, n);
                        value
                    }
                })
            });
        }
    }

    // Completely remove a Match from the matches
    pub fn remove_match(&mut self, m: &Match) {
        if let Some(set) = self.matches.get(m) {
            for t in set {
                self.ordered_times.remove(t);
            }
            self.matches.remove(m);
        }
    }

    /// Completely remove a Match from the triggers
    pub fn remove_trigger(&mut self, m: &Match) {
        self.triggers.remove(m);
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    pub fn decrement_trigger(&mut self, m: &Match, t: Time) -> bool {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            let mut exec_needed = false;
            let mt = MatchTime { m: m.clone(), t };
            let count = self
                .triggers
                .get(&mt.m)
                .and_then(|map| map.get(&mt.t))
                .cloned();
            if let Some(count) = count {
                exec_needed = true;
                if count <= 1 {
                    self.triggers.fetch_update(mt.m, |map| {
                        map.and_then(|mut map| {
                            map.remove(&mt.t);
                            if map.is_empty() {
                                None
                            } else {
                                Some(map)
                            }
                        })
                    });
                } else {
                    self.triggers.fetch_update(mt.m, |map| {
                        map.map(|mut map| {
                            map.insert(mt.t, count - 1);
                            map
                        })
                    });
                }
            }
            exec_needed
        } else {
            true
        }
    }

    pub fn clear_past_matches(&mut self, now: Time) {
        let retry_duration = self.filter.retry_duration.unwrap_or_default();
        while self
            .ordered_times
            .first_key_value()
            .is_some_and(|(t, _)| *t + retry_duration < now)
        {
            #[allow(clippy::unwrap_used)]
            // unwrap: we just checked in the condition that first is_some
            let (t, m) = {
                let (t, m) = self.ordered_times.first_key_value().unwrap();
                (*t, m.clone())
            };
            self.ordered_times.remove(&t);
            if let Some(set) = self.matches.get(&m) {
                let mut set = set.clone();
                set.remove(&t);
                if set.is_empty() {
                    self.matches.remove(&m);
                } else {
                    self.matches.insert(m, set);
                }
            }
        }
    }

    pub fn get_times(&self, m: &Match) -> usize {
        match self.matches.get(m) {
            Some(vec) => vec.len(),
            None => 0,
        }
    }

    fn load_matches_from_ordered_times(&mut self) {
        for (t, m) in self.ordered_times.iter() {
            let set = self.matches.entry(m.clone()).or_default();
            set.insert(*t);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use chrono::{DateTime, Local, TimeDelta};

    use crate::{
        concepts::{filter_tests::ok_filter, Action, Duplicate, Filter, Pattern},
        daemon::filter::state::State,
        tests::TempDatabase,
    };

    // Tests `new`, `clear_past_matches` and `load_matches_from_ordered_times`
    #[tokio::test]
    async fn state_new() {
        let patterns = Pattern::new_map("az", "[a-z]+").unwrap();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["true"],
                None,
                false,
                "s1",
                "f1",
                "a1",
                &patterns,
                0,
            )],
            vec!["test <az>"],
            Some(3),
            Some("2s"),
            "s1",
            "f1",
            Duplicate::default(),
            &patterns,
        );

        let now = DateTime::parse_from_rfc3339("2025-07-10T12:35:00.000+00:00")
            .unwrap()
            .with_timezone(&Local);
        let now_plus_1m = now + TimeDelta::minutes(1);
        let now_plus_1m01 = now_plus_1m + TimeDelta::seconds(1);
        let now_less_1m = now - TimeDelta::minutes(1);
        let now_less_1s = now - TimeDelta::seconds(1);
        let now_less_4s = now - TimeDelta::seconds(4);
        let now_less_5s = now - TimeDelta::seconds(5);

        let mut db = TempDatabase::default().await;
        db.set_loaded_db(HashMap::from([(
            "filter_ordered_times_s1.f1".into(),
            HashMap::from([
                // Will stay
                (now_plus_1m.to_rfc3339().into(), ["one"].into()),
                (now_plus_1m01.to_rfc3339().into(), ["one"].into()),
                (now_less_1s.to_rfc3339().into(), ["two"].into()), // stays because retry: 2s
                // Will get cleaned
                (now_less_4s.to_rfc3339().into(), ["two"].into()),
                (now_less_5s.to_rfc3339().into(), ["three"].into()),
                (now_less_1m.to_rfc3339().into(), ["two"].into()),
            ]),
        )]));

        let state = State::new(filter, &mut db, now).unwrap();

        assert_eq!(
            state.ordered_times.tree(),
            &BTreeMap::from([
                (now_less_1s, vec!["two".into()]),
                (now_plus_1m, vec!["one".into()]),
                (now_plus_1m01, vec!["one".into()]),
            ])
        );
        assert_eq!(
            state.matches,
            BTreeMap::from([
                (vec!["one".into()], [now_plus_1m, now_plus_1m01].into()),
                (vec!["two".into()], [now_less_1s].into()),
            ])
        );
    }

    #[tokio::test]
    async fn state_match_add_remove() {
        let filter = Box::leak(Box::new(ok_filter()));

        let one = vec!["one".into()];

        let now = DateTime::parse_from_rfc3339("2025-07-10T12:35:00.000+00:00")
            .unwrap()
            .with_timezone(&Local);
        let now_less_1s = now - TimeDelta::seconds(1);
        let now_less_4s = now - TimeDelta::seconds(4);

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).unwrap();

        assert!(state.ordered_times.tree().is_empty());
        assert!(state.matches.is_empty());

        // Add non-previously added match
        state.add_match(one.clone(), now_less_1s);
        assert_eq!(
            state.ordered_times.tree(),
            &BTreeMap::from([(now_less_1s, one.clone()),])
        );
        assert_eq!(
            state.matches,
            BTreeMap::from([(one.clone(), [now_less_1s].into())])
        );

        // Add previously added match
        state.add_match(one.clone(), now_less_4s);
        assert_eq!(
            state.ordered_times.tree(),
            &BTreeMap::from([(now_less_1s, one.clone()), (now_less_4s, one.clone())])
        );
        assert_eq!(
            state.matches,
            BTreeMap::from([(one.clone(), [now_less_1s, now_less_4s].into())])
        );

        // Remove added match
        state.remove_match(&one);
        assert!(state.ordered_times.tree().is_empty());
        assert!(state.matches.is_empty());
    }

    #[tokio::test]
    async fn state_trigger_no_after_add_remove_decrement() {
        let filter = Box::leak(Box::new(ok_filter()));

        let one = vec!["one".into()];
        let now = Local::now();

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).unwrap();

        assert!(state.triggers.tree().is_empty());

        // Add unique trigger
        state.add_trigger(one.clone(), now);
        // Nothing is really added
        assert!(state.triggers.tree().is_empty());

        // Will be called immediately after, it returns true
        assert!(state.decrement_trigger(&one, now));
    }

    #[tokio::test]
    async fn state_trigger_has_after_add_remove_decrement() {
        let patterns = Pattern::new_map("az", "[a-z]+").unwrap();
        let filter = Filter::new_static(
            vec![
                Action::new(vec!["true"], None, false, "s1", "f1", "a1", &patterns, 0),
                Action::new(
                    vec!["true"],
                    Some("1s"),
                    false,
                    "s1",
                    "f1",
                    "a2",
                    &patterns,
                    0,
                ),
                Action::new(
                    vec!["true"],
                    Some("3s"),
                    false,
                    "s1",
                    "f1",
                    "a3",
                    &patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            Some(3),
            Some("2s"),
            "s1",
            "f1",
            Duplicate::default(),
            &patterns,
        );

        let one = vec!["one".into()];
        let now = Local::now();
        let now_plus_1s = now + TimeDelta::seconds(1);

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).unwrap();

        assert!(state.triggers.tree().is_empty());

        // Add unique trigger
        state.add_trigger(one.clone(), now);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 3)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 2)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 1)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert!(state.triggers.tree().is_empty());
        // Decrement → false
        assert!(!state.decrement_trigger(&one, now));

        // Add trigger with neighbour
        state.add_trigger(one.clone(), now);
        state.add_trigger(one.clone(), now_plus_1s);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 3)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 2)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 1)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now));
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3)].into())])
        );
        // Decrement → false
        assert!(!state.decrement_trigger(&one, now));
        // Remove neighbour
        state.remove_trigger(&one);
        assert!(state.triggers.tree().is_empty());

        // Add two neighbour triggers
        state.add_trigger(one.clone(), now);
        state.add_trigger(one.clone(), now_plus_1s);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 3)].into())])
        );
        // Remove them
        state.remove_trigger(&one);
        assert!(state.triggers.tree().is_empty());
    }
}
