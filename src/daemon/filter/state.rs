use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use treedb::{Database, Tree, helpers::*};

use crate::concepts::{Filter, Match, MatchTime, Time};

pub fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name, filter.name
    )
}

pub fn filter_triggers_old_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name, filter.name)
}

pub fn filter_triggers_db_name(filter: &Filter) -> String {
    format!("filter_triggers2_{}.{}", filter.stream_name, filter.name)
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
    /// use reaction::concepts::now;
    ///
    /// let mut res = vec![];
    /// for _ in 0..10 {
    ///     let now = now();
    ///     res.push(format!("Now: {}", now.as_nanos()));
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
    pub async fn new(
        filter: &'static Filter,
        db: &mut Database,
        now: Time,
    ) -> Result<Self, String> {
        let ordered_times = db
            .open_tree(
                filter_ordered_times_db_name(filter),
                filter.retry_duration.unwrap_or_default(),
                |(key, value)| Ok((to_time(&key)?, to_match(&value)?)),
            )
            .await?;
        let mut triggers = db
            .open_tree(
                filter_triggers_db_name(filter),
                filter.longuest_action_duration,
                |(key, value)| Ok((to_match(&key)?, to_timemap(&value)?)),
            )
            .await?;
        if triggers.is_empty() {
            let old_triggers = db
                .open_tree(
                    filter_triggers_old_db_name(filter),
                    filter.longuest_action_duration,
                    |(key, value)| Ok((to_matchtime(&key)?, to_u64(&value)?)),
                )
                .await?;
            for (mt, n) in old_triggers.iter() {
                triggers
                    .fetch_update(mt.m.clone(), |map| {
                        Some(match map {
                            None => [(mt.t, *n)].into(),
                            Some(mut map) => {
                                map.insert(mt.t, *n);
                                map
                            }
                        })
                    })
                    .await;
            }
        }
        let mut this = Self {
            filter,
            has_after: !filter.longuest_action_duration.is_zero(),
            matches: BTreeMap::new(),
            ordered_times,
            triggers,
        };
        this.clear_past_matches(now).await;
        this.load_matches_from_ordered_times().await;
        Ok(this)
    }

    pub async fn add_match(&mut self, m: Match, t: Time) {
        let set = self.matches.entry(m.clone()).or_default();
        set.insert(t);
        self.ordered_times.insert(t, m).await;
    }

    pub async fn add_trigger(&mut self, m: Match, t: Time, action_count: Option<u64>) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            let n = action_count
                .unwrap_or_else(|| self.filter.filtered_actions_from_match(&m).len() as u64);
            self.triggers
                .fetch_update(m, |map| {
                    Some(match map {
                        None => [(t, n)].into(),
                        Some(mut value) => {
                            value.insert(t, n);
                            value
                        }
                    })
                })
                .await;
        }
    }

    // Completely remove a Match from the matches
    pub async fn remove_match(&mut self, m: &Match) {
        if let Some(set) = self.matches.get(m) {
            for t in set {
                self.ordered_times.remove(t).await;
            }
            self.matches.remove(m);
        }
    }

    /// Completely remove a Match from the triggers
    pub async fn remove_trigger(&mut self, m: &Match) -> Option<BTreeMap<Time, u64>> {
        self.triggers.remove(m).await
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    pub async fn decrement_trigger(&mut self, m: &Match, t: Time, exiting: bool) -> bool {
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
                    if !exiting {
                        self.triggers
                            .fetch_update(mt.m, |map| {
                                map.and_then(|mut map| {
                                    map.remove(&mt.t);
                                    if map.is_empty() { None } else { Some(map) }
                                })
                            })
                            .await;
                    }
                    // else don't do anything
                    // Because that will remove the entry in the DB, and make
                    // it forget this trigger.
                    // Maybe we should have 2 maps for triggers:
                    // - The current for action counting, not persisted
                    // - Another like ordered_times, Tree<Time, Match>, persisted
                } else {
                    self.triggers
                        .fetch_update(mt.m, |map| {
                            map.map(|mut map| {
                                map.insert(mt.t, count - 1);
                                map
                            })
                        })
                        .await;
                }
            }
            exec_needed
        } else {
            true
        }
    }

    pub async fn clear_past_matches(&mut self, now: Time) {
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
            self.ordered_times.remove(&t).await;
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

    pub async fn get_times(&self, m: &Match) -> usize {
        match self.matches.get(m) {
            Some(vec) => vec.len(),
            None => 0,
        }
    }

    async fn load_matches_from_ordered_times(&mut self) {
        for (t, m) in self.ordered_times.iter() {
            let set = self.matches.entry(m.clone()).or_default();
            set.insert(*t);
        }
    }
}

/// Tries to convert a [`Value`] into a [`MatchTime`]
pub fn to_matchtime(val: &Value) -> Result<MatchTime, String> {
    let map = val.as_object().ok_or("not an object")?;
    Ok(MatchTime {
        m: to_match(map.get("m").ok_or("no m in object")?)?,
        t: to_time(map.get("t").ok_or("no t in object")?)?,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use serde_json::{Map, Value, json};

    use crate::{
        concepts::{
            Action, Duplicate, Filter, MatchTime, Pattern, Time, filter_tests::ok_filter, now,
        },
        tests::TempDatabase,
    };

    use super::{State, to_matchtime};

    // Tests `new`, `clear_past_matches` and `load_matches_from_ordered_times`
    #[tokio::test]
    async fn state_new() {
        let patterns = Pattern::new_map("az", "[a-z]+").unwrap();
        let filter = Filter::new_static(
            vec![
                Action::new(vec!["true"], None, false, "s1", "f1", "a1", &patterns, 0),
                Action::new(
                    vec!["true"],
                    Some("3s"),
                    false,
                    "s1",
                    "f1",
                    "a2",
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

        let now = Time::from_secs(1234567);
        // DateTime::parse_from_rfc3339("2025-07-10T12:35:00.000+00:00")
        // .unwrap()
        // .with_timezone(&Local);
        let now_plus_1m = now + Time::from_mins(1);
        let now_plus_1m01 = now_plus_1m + Time::from_secs(1);
        let now_less_1m = now - Time::from_mins(1);
        let now_less_1s = now - Time::from_secs(1);
        let now_less_4s = now - Time::from_secs(4);
        let now_less_5s = now - Time::from_secs(5);

        let triggers = [
            // format v1
            (
                "filter_triggers_s1.f1".into(),
                HashMap::from([
                    // Will stay
                    (
                        json!({
                            "t": now_plus_1m,
                            "m": ["one"],
                        }),
                        json!(1),
                    ),
                    (
                        json!({
                            "t": now_less_1s,
                            "m": ["one"],
                        }),
                        json!(1),
                    ),
                    // Will not get cleaned because it's FilterManager's task
                    (
                        json!({
                            "t": now_less_5s,
                            "m": ["one"],
                        }),
                        json!(1),
                    ),
                ]),
            ),
            // format v2 (since v2.2.0)
            (
                "filter_triggers2_s1.f1".into(),
                HashMap::from([(
                    json!(["one"]),
                    json!({
                        // Will stay
                        now_plus_1m.as_nanos().to_string(): 1,
                        now_less_1s.as_nanos().to_string(): 1,
                    // Will not get cleaned because it's FilterManager's task
                        now_less_5s.as_nanos().to_string(): 1,
                    }),
                )]),
            ),
        ];

        for trigger_db in triggers {
            let mut db = TempDatabase::from_loaded_db(HashMap::from([
                (
                    "filter_ordered_times_s1.f1".into(),
                    HashMap::from([
                        // Will stay
                        (now_plus_1m.as_nanos().to_string().into(), ["one"].into()),
                        (now_plus_1m01.as_nanos().to_string().into(), ["one"].into()),
                        (now_less_1s.as_nanos().to_string().into(), ["two"].into()), // stays because retry: 2s
                        // Will get cleaned
                        (now_less_4s.as_nanos().to_string().into(), ["two"].into()),
                        (now_less_5s.as_nanos().to_string().into(), ["three"].into()),
                        (now_less_1m.as_nanos().to_string().into(), ["two"].into()),
                    ]),
                ),
                trigger_db,
            ]))
            .await;

            let state = State::new(filter, &mut db, now).await.unwrap();

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
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(
                    vec!["one".into()],
                    BTreeMap::from([
                        (now_less_5s, 1u64),
                        (now_less_1s, 1u64),
                        (now_plus_1m, 1u64),
                    ]),
                )])
            );
        }
    }

    #[tokio::test]
    async fn state_match_add_remove() {
        let filter = Box::leak(Box::new(ok_filter()));

        let one = vec!["one".into()];

        let now = Time::from_secs(1234567);
        let now_less_1s = now - Time::from_secs(1);
        let now_less_4s = now - Time::from_secs(4);

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).await.unwrap();

        assert!(state.ordered_times.tree().is_empty());
        assert!(state.matches.is_empty());

        // Add non-previously added match
        state.add_match(one.clone(), now_less_1s).await;
        assert_eq!(
            state.ordered_times.tree(),
            &BTreeMap::from([(now_less_1s, one.clone()),])
        );
        assert_eq!(
            state.matches,
            BTreeMap::from([(one.clone(), [now_less_1s].into())])
        );

        // Add previously added match
        state.add_match(one.clone(), now_less_4s).await;
        assert_eq!(
            state.ordered_times.tree(),
            &BTreeMap::from([(now_less_1s, one.clone()), (now_less_4s, one.clone())])
        );
        assert_eq!(
            state.matches,
            BTreeMap::from([(one.clone(), [now_less_1s, now_less_4s].into())])
        );

        // Remove added match
        state.remove_match(&one).await;
        assert!(state.ordered_times.tree().is_empty());
        assert!(state.matches.is_empty());
    }

    #[tokio::test]
    async fn state_trigger_no_after_add_remove_decrement() {
        let filter = Box::leak(Box::new(ok_filter()));

        let one = vec!["one".into()];
        let now = now();

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).await.unwrap();

        assert!(state.triggers.tree().is_empty());

        // Add unique trigger
        state.add_trigger(one.clone(), now, None).await;
        // Nothing is really added
        assert!(state.triggers.tree().is_empty());

        // Will be called immediately after, it returns true
        assert!(state.decrement_trigger(&one, now, false).await);
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
        let now = now();
        let now_plus_1s = now + Time::from_secs(1);

        let mut db = TempDatabase::default().await;
        let mut state = State::new(filter, &mut db, now).await.unwrap();

        assert!(state.triggers.tree().is_empty());

        // Add unique trigger
        state.add_trigger(one.clone(), now, None).await;
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 3)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 2)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 1)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert!(state.triggers.tree().is_empty());
        // Decrement → false
        assert!(!state.decrement_trigger(&one, now, false).await);

        // Add unique trigger (but decrement exiting-like)
        state.add_trigger(one.clone(), now, None).await;
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 3)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, true).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 2)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, true).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 1)].into())])
        );
        // Decrement but exiting → true, does nothing
        assert!(state.decrement_trigger(&one, now, true).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now, 1)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert!(state.triggers.tree().is_empty());
        // Decrement → false
        assert!(!state.decrement_trigger(&one, now, false).await);

        // Add trigger with neighbour
        state.add_trigger(one.clone(), now, None).await;
        state.add_trigger(one.clone(), now_plus_1s, None).await;
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 3)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 2)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 1)].into())])
        );
        // Decrement → true
        assert!(state.decrement_trigger(&one, now, false).await);
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3)].into())])
        );
        // Decrement → false
        assert!(!state.decrement_trigger(&one, now, false).await);
        // Remove neighbour
        state.remove_trigger(&one).await;
        assert!(state.triggers.tree().is_empty());

        // Add two neighbour triggers
        state.add_trigger(one.clone(), now, None).await;
        state.add_trigger(one.clone(), now_plus_1s, None).await;
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(one.clone(), [(now_plus_1s, 3), (now, 3)].into())])
        );
        // Remove them
        state.remove_trigger(&one).await;
        assert!(state.triggers.tree().is_empty());
    }

    #[test]
    fn test_to_matchtime() {
        assert_eq!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([
                    ("m".into(), ["plip", "ploup"].into()),
                    ("t".into(), "12345678".into()),
                ])
                .into_iter()
            ))),
            Ok(MatchTime {
                m: vec!["plip".into(), "ploup".into()],
                t: Time::from_nanos(12345678),
            })
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("m".into(), ["plip", "ploup"].into()),]).into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("t".into(), 12345678.into()),]).into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([("m".into(), "ploup".into()), ("t".into(), 12345678.into()),])
                    .into_iter()
            )))
            .is_err()
        );

        assert!(
            to_matchtime(&Value::Object(Map::from_iter(
                BTreeMap::from([
                    ("m".into(), ["plip", "ploup"].into()),
                    ("t".into(), [1234567].into()),
                ])
                .into_iter()
            )))
            .is_err()
        );
    }
}
