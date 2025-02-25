use std::{
    collections::{BTreeMap, BTreeSet},
    process::Stdio,
    sync::Arc,
};

use regex::Regex;
use tokio::sync::Semaphore;
use tracing::{error, info};

use crate::{
    concepts::{Action, Filter, Match, Pattern, Time},
    protocol::{Order, PatternStatus},
};

use super::{shutdown::ShutdownToken, SledDbExt, Tree};

#[derive(Clone)]
pub struct FilterManager {
    /// the Filter managed
    filter: &'static Filter,
    /// Has the filter at least an action with an after directive?
    has_after: bool,
    /// Permits to limit concurrency of actions execution
    exec_limit: Option<Arc<Semaphore>>,
    /// Permits to run pending actions on shutdown
    shutdown: ShutdownToken,
    /// Saves all the current Matches for this Filter
    matches: Tree<Match, BTreeSet<Time>>,
    /// Alternative view of the current Matches for O(1) cleaning of old Matches
    /// without added async Tasks to remove them
    ordered_times: Tree<Time, Match>,
    /// Saves all the current Triggers for this Filter
    triggers: Tree<Match, BTreeMap<Time, usize>>,
}

#[allow(clippy::unwrap_used)]
impl FilterManager {
    pub fn new(
        filter: &'static Filter,
        exec_limit: Option<Arc<Semaphore>>,
        shutdown: ShutdownToken,
        db: &sled::Db,
        now: Time,
    ) -> Result<Self, sled::Error> {
        let manager = Self {
            filter,
            has_after: !filter.longuest_action_duration().is_zero(),
            exec_limit,
            shutdown,
            matches: db.open_filter_matches_tree(filter)?,
            ordered_times: db.open_filter_ordered_times_tree(filter)?,
            triggers: db.open_filter_triggers_tree(filter)?,
        };
        manager.clear_past_matches(now);
        manager.clear_past_triggers_and_schedule_future_actions(now);
        Ok(manager)
    }

    pub fn handle_line(&self, line: &str, now: Time) -> bool {
        if let Some(match_) = self.filter.get_match(line) {
            self.handle_match(match_, now);
            true
        } else {
            false
        }
    }

    fn handle_match(&self, m: Match, now: Time) {
        self.clear_past_matches(now);

        let exec = match self.filter.retry() {
            None => true,
            Some(retry) => {
                self.add_match(&m, now);
                // Number of stored times for this match >= configured retry for this filter
                self.get_times(&m) >= retry as usize
            }
        };

        if exec {
            self.remove_match(&m);
            self.add_trigger(&m, now);
            self.schedule_exec(m.clone(), now, now);
        }
    }

    pub fn handle_order(
        &self,
        patterns: &BTreeMap<Arc<Pattern>, Regex>,
        order: Order,
        now: Time,
    ) -> BTreeMap<String, PatternStatus> {
        let is_match = |match_: &Match| {
            match_
                .iter()
                .zip(self.filter.patterns())
                .filter_map(|(a_match, pattern)| {
                    patterns.get(pattern.as_ref()).map(|regex| (a_match, regex))
                })
                .all(|(a_match, regex)| regex.is_match(a_match))
        };

        let mut cs: BTreeMap<_, _> = self
            .matches
            .iter()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
            .map(|(match_, times)| {
                if let Order::Flush = order {
                    self.remove_match(&match_);
                }
                (
                    match_,
                    PatternStatus {
                        matches: times.len(),
                        ..Default::default()
                    },
                )
            })
            .collect();

        for (match_, times) in self
            .triggers
            .iter()
            // match filtering
            .filter(|(match_, _)| is_match(match_))
        {
            // Remove the match from the triggers
            if let Order::Flush = order {
                self.remove_trigger(&match_);
            }

            let pattern_status = cs.entry(match_.clone()).or_default();

            for action in self.filter.actions().values() {
                let mut action_times = Vec::new();
                for time in times.keys() {
                    let action_time = *time + action.after_duration().unwrap_or_default();
                    if action_time > now {
                        action_times.push(action_time.to_rfc3339().chars().take(19).collect());
                        // Execute the action early
                        if let Order::Flush = order {
                            self.exec_now(action, match_.clone());
                        }
                    }
                }
                if !action_times.is_empty() {
                    pattern_status
                        .actions
                        .insert(action.name().into(), action_times);
                }
            }
        }

        cs.into_iter().map(|(k, v)| (k.join(" "), v)).collect()
    }

    /// Schedule execution for a given Action and Match.
    /// We check first if the trigger is still here
    /// because pending actions can be flushed.
    fn schedule_exec(&self, m: Match, t: Time, now: Time) {
        for action in self.filter.actions().values() {
            let exec_time = t + action.after_duration().unwrap_or_default();
            let m = m.clone();

            if exec_time <= now {
                if self.decrement_trigger(&m, t) {
                    self.exec_now(action, m);
                }
            } else {
                let this = self.clone();
                tokio::spawn(async move {
                    let dur = (exec_time - now)
                        .to_std()
                        // Could cause an error if t + after < now
                        // In this case, 0 is fine
                        .unwrap_or_default();
                    // Wait either for end of sleep
                    // or reaction exiting
                    let exiting = tokio::select! {
                        _ = tokio::time::sleep(dur) => false,
                        _ = this.shutdown.wait() => true,
                    };
                    // Exec action if triggered hasn't been already flushed
                    if (!exiting || action.on_exit()) && this.decrement_trigger(&m, t) {
                        this.exec_now(action, m);
                    }
                });
            }
        }
    }

    fn add_match(&self, m: &Match, t: Time) {
        // FIXME do this in a transaction
        self.matches
            .fetch_and_update(m, |set| {
                let mut set = set.unwrap_or_default();
                set.insert(t);
                Some(set)
            })
            .unwrap();
        self.ordered_times.insert(&t, m).unwrap();
    }

    fn add_trigger(&self, m: &Match, t: Time) {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            // Add the (Match, Time) to the triggers map
            self.triggers
                .fetch_and_update(m, |map| {
                    let mut map = map.unwrap_or_default();
                    map.insert(t, self.filter.actions().len());
                    Some(map)
                })
                .unwrap();
        }
    }

    // Completely remove a Match from the matches
    fn remove_match(&self, m: &Match) {
        // FIXME do this in a transaction
        if let Some(times) = self.matches.remove(m) {
            for t in times {
                self.ordered_times.remove(&t);
            }
        }
    }

    /// Completely remove a Match from the triggers
    fn remove_trigger(&self, m: &Match) {
        // FIXME do this in a transaction
        self.triggers.remove(m);
    }

    /// Returns whether we should still execute an action for this (Match, Time) trigger
    fn decrement_trigger(&self, m: &Match, t: Time) -> bool {
        // We record triggered filters only when there is an action with an `after` directive
        if self.has_after {
            let mut exec_needed = false;
            self.triggers
                .fetch_and_update(m, |map| {
                    map.map(|mut map| {
                        if let Some(counter) = map.get(&t) {
                            exec_needed = true;
                            // We're the last action to run
                            if *counter <= 1 {
                                map.remove(&t);
                            } else {
                                map.insert(t, counter - 1);
                            }
                        }
                        map
                    })
                    // Remove empty maps
                    .filter(|map| !map.is_empty())
                })
                .unwrap();
            exec_needed
        } else {
            true
        }
    }

    fn clear_past_matches(&self, now: Time) {
        let retry_duration = self.filter.retry_duration().unwrap_or_default();
        while self
            .ordered_times
            .first()
            .unwrap()
            .is_some_and(|(t, _)| t + retry_duration < now)
        {
            // FIXME do this in a transaction
            #[allow(clippy::unwrap_used)]
            // second unwrap: we just checked in the condition that first is_some
            let (t, m) = self.ordered_times.pop_min().unwrap().unwrap();
            self.matches
                .fetch_and_update(&m, |set| {
                    let mut set = set.unwrap();
                    set.remove(&t);
                    Some(set)
                })
                .unwrap();
        }
    }

    fn get_times(&self, m: &Match) -> usize {
        self.matches.get(m).unwrap().map(|v| v.len()).unwrap_or(0)
    }

    fn clear_past_triggers_and_schedule_future_actions(&self, now: Time) {
        let longuest_action_duration = self.filter.longuest_action_duration();
        let number_of_actions = self.filter.actions().len();

        for (m, map) in self.triggers.iter() {
            let new_map: BTreeMap<_, _> = map
                .into_iter()
                // Keep only times that are still relevant
                .filter(|(t, _)| *t + longuest_action_duration > now)
                // Reset the action counter
                .map(|(t, _)| (t, number_of_actions))
                .collect();

            if new_map.is_empty() {
                // No upcoming time, delete the entry from the Tree
                self.triggers.remove(&m);
            } else {
                // Insert back the upcoming times
                let _ = self.triggers.insert(&m, &new_map);

                // Schedule the upcoming times
                for t in new_map.into_keys() {
                    self.schedule_exec(m.clone(), t, now);
                }
            }
        }
    }

    fn exec_now(&self, action: &'static Action, m: Match) {
        let exec_limit = self.exec_limit.clone();
        tokio::spawn(async move {
            // Wait for semaphore's permission, if it is Some
            let _permit = match exec_limit {
                #[allow(clippy::unwrap_used)] // We know the semaphore is not closed
                Some(semaphore) => Some(semaphore.acquire_owned().await.unwrap()),
                None => None,
            };

            // Construct command
            let mut command = action.exec(&m);

            info!("{}: run [{:?}]", &action, command.as_std());
            if let Err(err) = command
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .stdout(Stdio::piped())
                .status()
                .await
            {
                error!("{}: run [{:?}], code {}", &action, command.as_std(), err);
            }
        });
    }
}

#[allow(clippy::unwrap_used)]
#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs::read_to_string,
        sync::Arc,
        time::Duration,
    };

    use chrono::{Local, TimeDelta};
    use tempfile::TempPath;
    use tokio::sync::Semaphore;

    use super::{FilterManager, SledDbExt};
    use crate::{
        concepts::{Action, Filter, Match, Pattern, Patterns, Time},
        daemon::{shutdown::ShutdownController, Tree},
    };

    struct TestBed {
        pub _out_path: TempPath,
        pub out_file: String,
        pub az_patterns: Patterns,
    }

    impl TestBed {
        fn new() -> Self {
            let _out_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
            let out_file = _out_path.to_str().unwrap().to_string();

            let az_patterns = Pattern::new_map("az", "[a-z]+").unwrap();
            Self {
                _out_path,
                out_file,
                az_patterns,
            }
        }

        fn part2(self, filter: &'static Filter, now: Time) -> TestBed2 {
            let db = crate::tests::TempDb::new();
            let controller = ShutdownController::new();
            let semaphore = Arc::new(Semaphore::new(1));
            TestBed2 {
                _out_path: self._out_path,
                out_file: self.out_file,
                now,
                matches: db.open_filter_matches_tree(filter).unwrap(),
                ordered_times: db.open_filter_ordered_times_tree(filter).unwrap(),
                triggers: db.open_filter_triggers_tree(filter).unwrap(),
                manager: FilterManager::new(
                    &filter,
                    Some(semaphore.clone()),
                    controller.token(),
                    &db,
                    now,
                )
                .unwrap(),
                // db,
                // controller,
                semaphore,
            }
        }
    }

    struct TestBed2 {
        pub _out_path: TempPath,
        pub out_file: String,
        // pub db: TempDb,
        // pub controller: ShutdownController,
        pub semaphore: Arc<Semaphore>,
        pub now: Time,
        pub matches: Tree<Match, BTreeSet<Time>>,
        pub ordered_times: Tree<Time, Match>,
        pub triggers: Tree<Match, BTreeMap<Time, usize>>,
        pub manager: FilterManager,
    }

    impl TestBed2 {
        fn assert_empty_trees(&self) {
            assert!(
                self.matches.iter().next().is_none(),
                "matches must be empty"
            );
            assert!(
                self.ordered_times.iter().next().is_none(),
                "ordered_times must be empty"
            );
            assert!(
                self.triggers.iter().next().is_none(),
                "triggers must be empty"
            );
        }
    }

    #[tokio::test]
    async fn three_matches_then_action_then_delayed_action() {
        let bed = TestBed::new();
        let filter = Filter::new_static(
            vec![
                Action::new(
                    vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                    None,
                    false,
                    "test",
                    "test",
                    "a1",
                    &bed.az_patterns,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("100ms"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                ),
            ],
            vec!["test <az>"],
            Some(3),
            Some("2s"),
            "test",
            "test",
            &bed.az_patterns,
        );

        let bed = bed.part2(filter, Local::now());

        let now = bed.now;
        let now1s = bed.now + TimeDelta::seconds(1);
        let now2s = bed.now + TimeDelta::seconds(2);

        // No match
        assert!(!bed.manager.handle_line("test 131", now));
        bed.assert_empty_trees();

        // First match
        assert!(bed.manager.handle_line("test one", now));
        let one = vec!["one".to_string()];
        assert_eq!(
            bed.matches.as_map(),
            BTreeMap::from([(one.clone(), BTreeSet::from([now]))]),
            "the match has been added to matches"
        );
        assert_eq!(
            bed.ordered_times.as_map(),
            BTreeMap::from([(now, one.clone())]),
            "the match has been added to ordered_times"
        );
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers is still empty"
        );

        // Second match
        assert!(bed.manager.handle_line("test one", now1s));
        assert_eq!(
            bed.matches.as_map(),
            BTreeMap::from([(one.clone(), BTreeSet::from([now, now1s]))]),
            "a second match is present in matches"
        );
        assert_eq!(
            bed.ordered_times.as_map(),
            BTreeMap::from([(now, one.clone()), (now1s, one.clone())]),
            "a second match is present in ordered_times"
        );
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers is still empty"
        );

        // Third match, exec
        let _block = bed.semaphore.acquire().await.unwrap();
        bed.manager.handle_line("test one", now2s);
        assert!(
            bed.matches.iter().next().is_none(),
            "matches are emptied after trigger"
        );
        assert!(
            bed.ordered_times.iter().next().is_none(),
            "ordered_times are emptied after trigger"
        );
        assert_eq!(
            bed.triggers.as_map(),
            BTreeMap::from([(one.clone(), BTreeMap::from([(now2s, 1)]))]),
            "triggers now contain the triggered match with 1 action left" // 1 and not 2 because the decrement_trigger() doesn't wait for the semaphore
        );
        drop(_block);

        // Now the first action executes
        tokio::time::sleep(Duration::from_millis(40)).await;
        // Check first action
        assert_eq!(
            bed.triggers.as_map(),
            BTreeMap::from([(one.clone(), BTreeMap::from([(now2s, 1)]))]),
            "triggers still contain the triggered match with 1 action left"
        );
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the first action"
        );

        // Now the second action executes
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Check second action
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers are empty again"
        );
        assert_eq!(
            "a1 one\na2 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the 2 actions"
        );

        bed.assert_empty_trees();
    }

    #[tokio::test]
    async fn one_match_one_action() {
        let bed = TestBed::new();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
            )],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            &bed.az_patterns,
        );

        let bed = bed.part2(filter, Local::now());
        let now = bed.now;

        // No match
        assert!(!bed.manager.handle_line("test 131", now));
        assert!(
            bed.matches.iter().next().is_none(),
            "matches must be initially empty"
        );
        assert!(
            bed.ordered_times.iter().next().is_none(),
            "ordered_times must be initially empty"
        );
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers must be initially empty"
        );

        // match
        assert!(bed.manager.handle_line("test one", now));
        assert!(bed.matches.iter().next().is_none(), "matches stay empty");
        assert!(
            bed.ordered_times.iter().next().is_none(),
            "ordered_times stay empty"
        );
        assert!(bed.triggers.iter().next().is_none(), "triggers stay empty");

        // the action executes
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the first action"
        );

        bed.assert_empty_trees();
    }

    #[tokio::test]
    async fn one_match_one_delayed_action() {
        let bed = TestBed::new();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                Some("100ms"),
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
            )],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            &bed.az_patterns,
        );

        let bed = bed.part2(filter, Local::now());
        let now = bed.now;

        // No match
        assert!(!bed.manager.handle_line("test 131", now));
        assert!(
            bed.matches.iter().next().is_none(),
            "matches must be initially empty"
        );
        assert!(
            bed.ordered_times.iter().next().is_none(),
            "ordered_times must be initially empty"
        );
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers must be initially empty"
        );

        // Match
        let one = vec!["one".to_string()];
        assert!(bed.manager.handle_line("test one", now));
        assert!(bed.matches.iter().next().is_none(), "matches stay empty");
        assert!(
            bed.ordered_times.iter().next().is_none(),
            "ordered_times stay empty"
        );
        assert_eq!(
            bed.triggers.as_map(),
            BTreeMap::from([(one.clone(), BTreeMap::from([(now, 1)]))]),
            "triggers still contain the triggered match with 1 action left"
        );
        assert_eq!(
            "",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file is empty"
        );

        // The action executes
        tokio::time::sleep(Duration::from_millis(140)).await;
        assert!(
            bed.triggers.iter().next().is_none(),
            "triggers are empty again"
        );
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action"
        );

        bed.assert_empty_trees();
    }

    // TODO test persistance, ie. FilterManagers created with non-empty db
}
