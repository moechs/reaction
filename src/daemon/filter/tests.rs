use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::read_to_string,
    ops::Add,
    sync::Arc,
    time::Duration,
};

use reaction_plugin::shutdown::ShutdownController;
use regex::Regex;
use serde_json::json;
use tempfile::TempPath;
use tokio::{sync::Semaphore, time::sleep};

use super::{
    FilterManager, React,
    state::{filter_ordered_times_db_name, filter_triggers_db_name},
};
use crate::{
    concepts::{Action, Duplicate, Filter, Pattern, Patterns, Time, now},
    daemon::plugin::Plugins,
    protocol::{Order, PatternStatus},
    tests::TempDatabase,
};

pub struct TestBed {
    pub _out_path: TempPath,
    pub out_file: String,
    pub az_patterns: Patterns,
    pub ip_patterns: Patterns,
}

impl Default for TestBed {
    fn default() -> Self {
        let _out_path = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        let out_file = _out_path.to_str().unwrap().to_string();

        let az_patterns = Pattern::new_map("az", "[a-z]+").unwrap();
        let ip_patterns = [
            "type: ip",
            "
type: ipv4
ignorecidr:
  - 192.168.1.0/24
",
            "
type: ipv4
ipv4mask: 24
ignorecidr:
  - 192.168.1.0/24
",
            "
type: ipv6
ignorecidr:
  - fe80::/16
",
            "
type: ipv6
ipv6mask: 64
ignorecidr:
  - 1::/120
",
        ]
        .into_iter()
        .map(serde_yaml::from_str::<Pattern>)
        .map(Result::unwrap)
        .zip(["ip", "ipv4", "ipv4_mask24", "ipv6", "ipv6_mask64"])
        .map(|(mut pat, name)| {
            pat.setup(name).unwrap();
            (name.into(), pat.into())
        })
        .collect();
        Self {
            _out_path,
            out_file,
            az_patterns,
            ip_patterns,
        }
    }
}

impl TestBed {
    pub async fn part2(
        self,
        filter: &'static Filter,
        now: Time,
        db: Option<TempDatabase>,
    ) -> TestBed2 {
        let mut db = match db {
            Some(db) => db,
            None => TempDatabase::default().await,
        };
        let controller = ShutdownController::new();
        let semaphore = Arc::new(Semaphore::new(1));
        let test_bed2 = TestBed2 {
            _out_path: self._out_path,
            out_file: self.out_file,
            az_patterns: self.az_patterns,
            ip_patterns: self.ip_patterns,
            now,
            manager: FilterManager::new(
                filter,
                Some(semaphore.clone()),
                controller.token(),
                &mut db,
                &mut Plugins::default(),
                now,
            )
            .await
            .unwrap(),
            semaphore,
        };
        test_bed2.manager.start(now).await;
        test_bed2
    }
}

pub struct TestBed2 {
    pub _out_path: TempPath,
    pub out_file: String,
    pub semaphore: Arc<Semaphore>,
    pub now: Time,
    pub manager: FilterManager,
    pub az_patterns: Patterns,
    pub ip_patterns: Patterns,
}

impl TestBed2 {
    pub async fn assert_empty_trees(&self) {
        let state = self.manager.state.lock().await;
        assert!(state.matches.is_empty(), "matches must be empty");
        assert!(
            state.ordered_times.is_empty(),
            "ordered_times must be empty"
        );
        assert!(state.triggers.is_empty(), "triggers must be empty");
    }

    pub async fn reset_out_file(&self) {
        tokio::fs::write(&self.out_file, "").await.unwrap();
    }
}

#[tokio::test]
async fn three_matches_then_action_then_delayed_action() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
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
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("100ms"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            Some(3),
            Some("2s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let bed = bed.part2(filter, now, None).await;

        let now = bed.now;
        let now1s = bed.now + Time::from_secs(1);
        let now2s = bed.now + Time::from_secs(2);

        // No match
        assert_eq!(
            bed.manager.handle_line("test 131", now).await,
            React::NoMatch
        );
        bed.assert_empty_trees().await;

        // First match
        let one = vec!["one".to_string()];
        assert_eq!(bed.manager.handle_line("test one", now).await, React::Match);
        {
            let state = bed.manager.state.lock().await;
            assert_eq!(
                state.matches,
                BTreeMap::from([(one.clone(), BTreeSet::from([now]))]),
                "the match has been added to matches"
            );
            assert_eq!(
                state.ordered_times.tree(),
                &BTreeMap::from([(now, one.clone())]),
                "the match has been added to ordered_times"
            );
            assert!(state.triggers.is_empty(), "triggers is still empty");
        }

        // Second match
        assert_eq!(
            bed.manager.handle_line("test one", now1s).await,
            React::Match
        );
        {
            let state = bed.manager.state.lock().await;
            assert_eq!(
                state.matches,
                BTreeMap::from([(one.clone(), BTreeSet::from([now, now1s]))]),
                "a second match is present in matches"
            );
            assert_eq!(
                state.ordered_times.tree(),
                &BTreeMap::from([(now, one.clone()), (now1s, one.clone())]),
                "a second match is present in ordered_times"
            );
            assert!(state.triggers.is_empty(), "triggers is still empty");
        }

        // Third match, exec
        let _block = bed.semaphore.acquire().await.unwrap();
        assert_eq!(
            bed.manager.handle_line("test one", now2s).await,
            React::Trigger
        );
        {
            let state = bed.manager.state.lock().await;
            assert!(
                state.matches.is_empty(),
                "matches are emptied after trigger"
            );
            assert!(
                state.ordered_times.is_empty(),
                "ordered_times are emptied after trigger"
            );
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(one.clone(), BTreeMap::from([(now2s, 1)]))]),
                // 1 and not 2 because the decrement_trigger() doesn't wait for the semaphore
                "triggers now contain the triggered match with 1 action left"
            );
        }
        drop(_block);

        // Now the first action executes
        sleep(Duration::from_millis(40)).await;
        // Check first action
        assert_eq!(
            bed.manager.state.lock().await.triggers.tree(),
            &BTreeMap::from([(one.clone(), BTreeMap::from([(now2s, 1)]))]),
            "triggers still contain the triggered match with 1 action left"
        );
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the first action"
        );

        // Now the second action executes
        sleep(Duration::from_millis(140)).await;
        // Check second action
        assert!(
            bed.manager.state.lock().await.triggers.is_empty(),
            "triggers are empty again"
        );
        assert_eq!(
            "a1 one\na2 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the 2 actions"
        );

        bed.assert_empty_trees().await;
    }
}

#[tokio::test]
async fn one_match_one_action() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
                0,
            )],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let bed = bed.part2(filter, now, None).await;

        // No match
        assert_eq!(
            bed.manager.handle_line("test 131", now).await,
            React::NoMatch
        );
        bed.assert_empty_trees().await;

        // match
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger
        );
        bed.assert_empty_trees().await;

        // the action executes
        sleep(Duration::from_millis(40)).await;
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the first action"
        );

        bed.assert_empty_trees().await;
    }
}

#[tokio::test]
async fn one_match_one_delayed_action() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                Some("100ms"),
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
                0,
            )],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let bed = bed.part2(filter, now, None).await;

        // No match
        assert_eq!(
            bed.manager.handle_line("test 131", now).await,
            React::NoMatch
        );
        bed.assert_empty_trees().await;

        // Match
        let one = vec!["one".to_string()];
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger
        );
        {
            let state = bed.manager.state.lock().await;
            assert!(state.matches.is_empty(), "matches stay empty");
            assert!(state.ordered_times.is_empty(), "ordered_times stay empty");
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(one.clone(), BTreeMap::from([(now, 1)]))]),
                "triggers still contain the triggered match with 1 action left"
            );
        }
        assert_eq!(
            "",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file is empty"
        );

        // The action executes
        sleep(Duration::from_millis(140)).await;
        assert!(
            bed.manager.state.lock().await.triggers.is_empty(),
            "triggers are empty again"
        );
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action"
        );

        bed.assert_empty_trees().await;
    }
}

#[tokio::test]
async fn one_db_match_one_runtime_match_one_action() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
                0,
            )],
            vec!["test <az>"],
            Some(2),
            Some("2s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        // Pre-add match
        let now = now();
        let one = vec!["one".to_string()];
        let now1s = now - Time::from_secs(1);

        let db = TempDatabase::from_loaded_db(HashMap::from([(
            filter_ordered_times_db_name(filter),
            HashMap::from([(now1s.as_nanos().to_string().into(), one.clone().into())]),
        )]))
        .await;

        // Finish setup
        let bed = bed.part2(filter, now, Some(db)).await;

        {
            let state = bed.manager.state.lock().await;
            assert_eq!(
                state.matches,
                BTreeMap::from([(one.clone(), BTreeSet::from([now1s]))]),
                "the match previously added to matches"
            );
            assert_eq!(
                state.ordered_times.tree(),
                &BTreeMap::from([(now1s, one.clone())]),
                "the match previously added to matches"
            );
            assert!(state.triggers.is_empty(), "triggers stay empty");
        }

        // match
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger
        );
        bed.assert_empty_trees().await;
        // the action executes
        sleep(Duration::from_millis(40)).await;
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action"
        );
    }
}

#[tokio::test]
async fn one_outdated_db_match() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
        let filter = Filter::new_static(
            vec![Action::new(
                vec!["sh", "-c", &format!("echo a1 <az> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a1",
                &bed.az_patterns,
                0,
            )],
            vec!["test <az>"],
            Some(2),
            Some("1s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        // Pre-add match
        let now = now();
        let one = vec!["one".to_string()];
        let now1s = now - Time::from_millis(1001);

        let db = TempDatabase::from_loaded_db(HashMap::from([(
            filter_ordered_times_db_name(filter),
            HashMap::from([(now1s.as_nanos().to_string().into(), one.clone().into())]),
        )]))
        .await;

        // Finish setup
        let bed = bed.part2(filter, now, Some(db)).await;
        bed.assert_empty_trees().await;
    }
}

#[tokio::test]
async fn flush() {
    // TODO multiple tests
}

#[tokio::test]
async fn trigger_unmatched_pattern() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
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
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("200ms"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            Some(2),
            Some("1s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let one = vec!["one".to_string()];
        let bed = bed.part2(filter, now, None).await;

        bed.manager
            .handle_trigger(
                // az_pattern: "one"
                bed.az_patterns
                    .values()
                    .cloned()
                    .map(|pattern| (pattern, one[0].clone()))
                    .collect(),
                now,
            )
            .await
            .unwrap();

        // the action executes
        sleep(Duration::from_millis(40)).await;

        // No matches, one action registered
        {
            let state = bed.manager.state.lock().await;
            assert!(state.matches.is_empty());
            assert!(state.ordered_times.is_empty());
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(one.clone(), BTreeMap::from([(now, 1)]))]),
            );
        }
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action"
        );
    }
}

#[tokio::test]
async fn trigger_matched_pattern() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
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
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("200ms"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            Some(2),
            Some("1s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let now1s = now - Time::from_millis(10);
        let one = vec!["one".to_string()];

        let db = TempDatabase::from_loaded_db(HashMap::from([(
            filter_ordered_times_db_name(filter),
            HashMap::from([(now1s.as_nanos().to_string().into(), one.clone().into())]),
        )]))
        .await;
        let bed = bed.part2(filter, now, Some(db)).await;

        bed.manager
            .handle_trigger(
                // az_pattern: "one"
                bed.az_patterns
                    .values()
                    .cloned()
                    .map(|pattern| (pattern, one[0].clone()))
                    .collect(),
                now,
            )
            .await
            .unwrap();

        // the action executes
        sleep(Duration::from_millis(40)).await;

        // No matches, one action registered
        {
            let state = bed.manager.state.lock().await;
            assert!(state.matches.is_empty());
            assert!(state.ordered_times.is_empty());
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(one.clone(), BTreeMap::from([(now, 1)]))]),
            );
        }
        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action"
        );
    }
}

#[tokio::test]
async fn trigger_deduplication_on_start() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
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
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("2s"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let now1s = now - Time::from_millis(1000);
        let now2s = now - Time::from_millis(1030);
        let one = vec!["one".to_string()];

        let db = TempDatabase::from_loaded_db(HashMap::from([(
            filter_triggers_db_name(filter),
            HashMap::from([(
                one.clone().into(),
                json!({
                    now1s.as_nanos().to_string(): 1,
                    now2s.as_nanos().to_string(): 1,
                }),
            )]),
        )]))
        .await;
        let bed = bed.part2(filter, now, Some(db)).await;

        // the action executes
        sleep(Duration::from_millis(50)).await;

        // No matches, one or two action·s registered
        {
            let state = bed.manager.state.lock().await;
            assert!(state.matches.is_empty());
            assert!(state.ordered_times.is_empty());
            assert_eq!(
                state.triggers.tree(),
                &BTreeMap::from([(
                    one.clone(),
                    match dup {
                        Duplicate::Extend => BTreeMap::from([(now1s, 1)]),
                        Duplicate::Ignore => BTreeMap::from([(now2s, 1)]),
                        Duplicate::Rerun => BTreeMap::from([(now1s, 1), (now2s, 1)]),
                    }
                )]),
                "the state contains one or two triggers. Duplicate: {dup:?}"
            );
        }
        assert_eq!(
            match dup {
                Duplicate::Ignore | Duplicate::Extend => "a1 one\n",
                Duplicate::Rerun => "a1 one\na1 one\n",
            },
            &read_to_string(&bed.out_file).unwrap(),
            "the output file contains the result of the action. Duplicate: {dup:?}"
        );
    }
}

#[tokio::test]
async fn multiple_triggers() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        let bed = TestBed::default();
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
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                    Some("200ms"),
                    false,
                    "test",
                    "test",
                    "a2",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            Some(2),
            Some("1s"),
            "test",
            "test",
            dup,
            &bed.az_patterns,
        );

        let now = now();
        let bed = bed.part2(filter, now, None).await;

        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Match,
            "Duplicate: {dup:?}"
        );
        let now = crate::concepts::now();
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger,
            "Duplicate: {dup:?}"
        );

        // Wait for first action to execute
        sleep(Duration::from_millis(50)).await;

        assert_eq!(
            "a1 one\n",
            &read_to_string(&bed.out_file).unwrap(),
            "Duplicate: {dup:?}"
        );

        sleep(Duration::from_millis(50)).await;

        let now = crate::concepts::now();
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            match dup {
                Duplicate::Ignore => React::Match,
                _ => React::Match,
            },
            "Duplicate: {dup:?}"
        );

        let now = crate::concepts::now();
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            match dup {
                Duplicate::Ignore => React::Match,
                _ => React::Trigger,
            },
            "Duplicate: {dup:?}"
        );

        // Wait for second action to execute
        sleep(Duration::from_millis(50)).await;

        assert_eq!(
            &read_to_string(&bed.out_file).unwrap(),
            match dup {
                Duplicate::Rerun => "a1 one\na1 one\n",
                _ => "a1 one\n",
            },
            "Duplicate: {dup:?}"
        );

        // Wait for first after action to execute
        sleep(Duration::from_millis(100)).await;

        assert_eq!(
            &read_to_string(&bed.out_file).unwrap(),
            match dup {
                Duplicate::Rerun => "a1 one\na1 one\na2 one\n",
                Duplicate::Extend => "a1 one\n",
                Duplicate::Ignore => "a1 one\na2 one\n",
            },
            "Duplicate: {dup:?}"
        );

        // Wait for second after action to execute
        sleep(Duration::from_millis(100)).await;

        assert_eq!(
            &read_to_string(&bed.out_file).unwrap(),
            match dup {
                Duplicate::Rerun => "a1 one\na1 one\na2 one\na2 one\n",
                Duplicate::Extend => "a1 one\na2 one\n",
                Duplicate::Ignore => "a1 one\na2 one\n",
            },
            "Duplicate: {dup:?}"
        );
    }
}
#[tokio::test]
async fn extend_trigger_multiple_after_actions() {
    let last_match_duration = Duration::from_millis(350);

    for (second_match_duration, first_action_list, last_action_list) in [
        (
            Duration::from_millis(50),
            vec!["a0", "a1"],
            vec!["d2", "c3", "a4"],
        ),
        (
            Duration::from_millis(150),
            vec!["a0", "a1", "d2"],
            vec!["c3", "a4"],
        ),
        (
            Duration::from_millis(250),
            vec!["a0", "a1", "d2", "c3"],
            vec!["a4"],
        ),
    ] {
        let bed = TestBed::default();
        let filter = Filter::new_static(
            vec![
                Action::new(
                    vec!["sh", "-c", &format!("echo a0 >> {}", &bed.out_file)],
                    None,
                    false,
                    "test",
                    "test",
                    "a0",
                    &bed.az_patterns,
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a1 >> {}", &bed.out_file)],
                    None,
                    false,
                    "test",
                    "test",
                    "a1",
                    &bed.az_patterns,
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo d2 >> {}", &bed.out_file)],
                    Some("100ms"),
                    false,
                    "test",
                    "test",
                    "d2",
                    &bed.az_patterns,
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo c3 >> {}", &bed.out_file)],
                    Some("200ms"),
                    false,
                    "test",
                    "test",
                    "c3",
                    &bed.az_patterns,
                    0,
                ),
                Action::new(
                    vec!["sh", "-c", &format!("echo a4 >> {}", &bed.out_file)],
                    Some("300ms"),
                    false,
                    "test",
                    "test",
                    "a4",
                    &bed.az_patterns,
                    0,
                ),
            ],
            vec!["test <az>"],
            None,
            None,
            "test",
            "test",
            Duplicate::Extend,
            &bed.az_patterns,
        );

        let now = now();
        let bed = bed.part2(filter, now, None).await;

        let now = crate::concepts::now();
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger,
        );

        // Wait for first trigger to execute (partially)
        sleep(second_match_duration).await;

        assert_eq!(
            first_action_list.join("\n"),
            read_to_string(&bed.out_file).unwrap().trim(),
            "Sleep: {second_match_duration:?}"
        );

        let now = crate::concepts::now();
        assert_eq!(
            bed.manager.handle_line("test one", now).await,
            React::Trigger,
        );

        // Wait for second trigger to execute (fully)
        sleep(last_match_duration).await;

        assert_eq!(
            first_action_list
                .into_iter()
                .chain(last_action_list.clone())
                .fold("".to_string(), |acc, elt| format!("{acc}\n{elt}"))
                .trim(),
            read_to_string(&bed.out_file).unwrap().trim(),
            "Sleep: {last_match_duration:?}"
        );
    }
}

#[tokio::test]
async fn ip_specific() {
    let bed = TestBed::default();
    let filter = Filter::new_static(
        vec![
            Action::new(
                vec!["sh", "-c", &format!("echo ipv4 <ip> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a4",
                &bed.ip_patterns,
                4,
            ),
            Action::new(
                vec!["sh", "-c", &format!("echo ipv6 <ip> >> {}", &bed.out_file)],
                None,
                false,
                "test",
                "test",
                "a6",
                &bed.ip_patterns,
                6,
            ),
            Action::new(
                vec!["sh", "-c", &format!("echo any <ip> >> {}", &bed.out_file)],
                Some("20ms"),
                false,
                "test",
                "test",
                "ax",
                &bed.ip_patterns,
                0,
            ),
        ],
        vec!["test <ip>"],
        None,
        None,
        "test",
        "test",
        Duplicate::Extend,
        &bed.ip_patterns,
    );

    let now = now();
    let bed = bed.part2(filter, now, None).await;

    let now = crate::concepts::now();
    assert_eq!(
        bed.manager.handle_line("test 1.2.3.4", now).await,
        React::Trigger,
    );

    // Wait for action to execute
    sleep(Duration::from_millis(70)).await;

    assert_eq!(
        "ipv4 1.2.3.4\nany 1.2.3.4\n",
        &read_to_string(&bed.out_file).unwrap(),
    );

    bed.reset_out_file().await;

    let now = crate::concepts::now();
    assert_eq!(
        bed.manager.handle_line("test 1:2:3:4:5:6:7:8", now).await,
        React::Trigger,
    );

    // Wait for action to execute
    sleep(Duration::from_millis(70)).await;

    assert_eq!(
        "ipv6 1:2:3:4:5:6:7:8\nany 1:2:3:4:5:6:7:8\n",
        &read_to_string(&bed.out_file).unwrap(),
    );
}

#[tokio::test]
async fn show_then_flush_one_match() {
    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        for query in [None, Some("one"), Some("o.*")] {
            let bed = TestBed::default();
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
                        0,
                    ),
                    Action::new(
                        vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                        Some("2s"),
                        false,
                        "test",
                        "test",
                        "a2",
                        &bed.az_patterns,
                        0,
                    ),
                ],
                vec!["test <az>"],
                Some(3),
                Some("3s"),
                "test",
                "test",
                dup,
                &bed.az_patterns,
            );

            let now = now();
            let now1s = now - Time::from_secs(1);
            let one = vec!["one".to_string()];

            let db = TempDatabase::from_loaded_db(HashMap::from([(
                filter_ordered_times_db_name(filter),
                HashMap::from([(now1s.as_nanos().to_string().into(), one.clone().into())]),
            )]))
            .await;
            let bed = bed.part2(filter, now, Some(db)).await;

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches,
                    BTreeMap::from([(vec!["one".into()], BTreeSet::from([now1s]))])
                );
            }

            let patterns = match query {
                Some(q) => &BTreeMap::from([(
                    bed.az_patterns.iter().next().unwrap().1.clone(),
                    Regex::new(q).unwrap(),
                )]),
                None => &BTreeMap::default(),
            };
            let result = BTreeMap::from([(
                "one".into(),
                PatternStatus {
                    matches: 1,
                    ..Default::default()
                },
            )]);

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Show, now).await,
                &result,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches,
                    BTreeMap::from([(vec!["one".into()], BTreeSet::from([now1s]))])
                );
            }

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Flush, now).await,
                &result,
            );

            {
                let state = bed.manager.state.lock().await;
                assert!(state.matches.is_empty());
            }
        }
    }
}

#[tokio::test]
async fn show_then_flush_matches() {
    let obaaa = vec!["obaaa".to_string()];
    let lama = vec!["lama".to_string()];
    let lou1uol = vec!["lou1uol".to_string()];
    let alulu = vec!["alulu".to_string()];
    let o1a = vec!["o1a".to_string()];

    let all = BTreeSet::from([
        obaaa.clone(),
        lama.clone(),
        lou1uol.clone(),
        alulu.clone(),
        o1a.clone(),
    ]);

    let now = now();
    let now1s = now - Time::from_secs(1);
    let now2s = now - Time::from_secs(2);
    let now3s = now - Time::from_secs(3);
    let now4s = now - Time::from_secs(4);
    let now5s = now - Time::from_secs(5);

    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        for (query, filtered) in [
            (Some("^o.*"), BTreeSet::from([obaaa.clone(), o1a.clone()])),
            (Some("1"), BTreeSet::from([lou1uol.clone(), o1a.clone()])),
            (
                Some("a$"),
                BTreeSet::from([obaaa.clone(), lama.clone(), o1a.clone()]),
            ),
            (None, all.clone()),
        ] {
            let bed = TestBed::default();
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
                        0,
                    ),
                    Action::new(
                        vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                        Some("10s"),
                        false,
                        "test",
                        "test",
                        "a2",
                        &bed.az_patterns,
                        0,
                    ),
                ],
                vec!["test <az>"],
                Some(3),
                Some("6s"),
                "test",
                "test",
                dup,
                &bed.az_patterns,
            );

            let db = TempDatabase::from_loaded_db(HashMap::from([(
                filter_ordered_times_db_name(filter),
                HashMap::from([
                    (now1s.as_nanos().to_string().into(), obaaa.clone().into()),
                    (now2s.as_nanos().to_string().into(), lama.clone().into()),
                    (now3s.as_nanos().to_string().into(), lou1uol.clone().into()),
                    (now4s.as_nanos().to_string().into(), alulu.clone().into()),
                    (now5s.as_nanos().to_string().into(), o1a.clone().into()),
                ]),
            )]))
            .await;
            let bed = bed.part2(filter, now, Some(db)).await;

            let patterns = match query {
                Some(q) => &BTreeMap::from([(
                    bed.az_patterns.iter().next().unwrap().1.clone(),
                    Regex::new(q).unwrap(),
                )]),
                None => &BTreeMap::default(),
            };
            let ps = PatternStatus {
                matches: 1,
                ..Default::default()
            };

            let filtered_map = filtered
                .iter()
                .map(|k| (k[0].clone(), ps.clone()))
                .collect();

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.iter().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Show, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.iter().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Flush, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.difference(&filtered).collect(),
                );
            }
        }
    }
}

#[tokio::test]
async fn show_then_flush_multiple_patterns() {
    let now = now();
    let now1s = now - Time::from_secs(1);
    let now2s = now - Time::from_secs(2);
    let now3s = now - Time::from_secs(3);
    let now4s = now - Time::from_secs(4);

    let m1a = vec!["aaa".to_string(), "1.1.1.1".to_string()];
    let m2a = vec!["aaa".to_string(), "2.2.2.2".to_string()];
    let m1b = vec!["b".to_string(), "1.1.1.1".to_string()];
    let m2b = vec!["b".to_string(), "2.2.2.2".to_string()];

    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        for (query, filtered) in [
            (
                (None, None),
                vec![m1a.clone(), m2a.clone(), m1b.clone(), m2b.clone()],
            ),
            ((Some("a"), None), vec![m1a.clone(), m2a.clone()]),
            ((None, Some("2.2.2.2")), vec![m2a.clone(), m2b.clone()]),
            ((Some("b"), Some("1.1.1.1")), vec![m1b.clone()]),
        ] {
            let bed = TestBed::default();

            let mut patterns = bed.az_patterns.clone();
            patterns.insert("ip".into(), bed.ip_patterns.get("ip").unwrap().clone());

            let filter = Filter::new_static(
                vec![
                    Action::new(
                        vec![
                            "sh",
                            "-c",
                            &format!("echo a1 <az> <ip> >> {}", &bed.out_file),
                        ],
                        None,
                        false,
                        "test",
                        "test",
                        "a1",
                        &patterns,
                        0,
                    ),
                    Action::new(
                        vec![
                            "sh",
                            "-c",
                            &format!("echo a2 <az> <ip> >> {}", &bed.out_file),
                        ],
                        Some("2s"),
                        false,
                        "test",
                        "test",
                        "a2",
                        &patterns,
                        0,
                    ),
                ],
                vec!["test <az> <ip>"],
                Some(3),
                Some("6s"),
                "test",
                "test",
                dup,
                &patterns,
            );

            let db = TempDatabase::from_loaded_db(HashMap::from([(
                filter_ordered_times_db_name(filter),
                HashMap::from([
                    (now1s.as_nanos().to_string().into(), m1a.clone().into()),
                    (now2s.as_nanos().to_string().into(), m2a.clone().into()),
                    (now3s.as_nanos().to_string().into(), m1b.clone().into()),
                    (now4s.as_nanos().to_string().into(), m2b.clone().into()),
                ]),
            )]))
            .await;
            let bed = bed.part2(filter, now, Some(db)).await;

            let patterns = {
                let mut patterns = BTreeMap::new();
                if let Some(az) = query.0 {
                    patterns.insert(
                        bed.az_patterns.iter().next().unwrap().1.clone(),
                        Regex::new(az).unwrap(),
                    );
                }
                if let Some(ip) = query.1 {
                    patterns.insert(
                        bed.ip_patterns.iter().next().unwrap().1.clone(),
                        Regex::new(ip).unwrap(),
                    );
                }
                patterns
            };
            let ps = PatternStatus {
                matches: 1,
                ..Default::default()
            };

            let all = BTreeSet::from([m1a.clone(), m2a.clone(), m1b.clone(), m2b.clone()]);

            let filtered = BTreeSet::from_iter(filtered.into_iter());
            let filtered_map = filtered.iter().map(|k| (k.join(" "), ps.clone())).collect();

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.iter().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(&patterns, Order::Show, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.iter().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(&patterns, Order::Flush, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.matches.keys().collect::<BTreeSet<_>>(),
                    all.difference(&filtered).collect(),
                );
            }
        }
    }
}

#[tokio::test]
async fn show_then_flush_triggers() {
    let now = now();
    let now1s = now - Time::from_secs(1);
    let now2s = now - Time::from_secs(2);
    let now3s = now - Time::from_secs(3);
    let now4s = now - Time::from_secs(4);
    let now5s = now - Time::from_secs(5);

    let obaaa = vec!["obaaa".to_string()];
    let lama = vec!["lama".to_string()];
    let lou1uol = vec!["lou1uol".to_string()];
    let alulu = vec!["alulu".to_string()];
    let o1a = vec!["o1a".to_string()];

    let all = BTreeMap::from([
        (obaaa.clone(), vec![now1s]),
        (lama.clone(), vec![now2s]),
        (lou1uol.clone(), vec![now3s]),
        (alulu.clone(), vec![now4s]),
        (o1a.clone(), vec![now5s]),
    ]);

    for dup in [Duplicate::Rerun, Duplicate::Extend, Duplicate::Ignore] {
        for (query, filtered) in [
            (
                Some("^o.*"),
                all.clone()
                    .into_iter()
                    .filter(|(k, _)| [&obaaa, &o1a].contains(&k))
                    .collect(),
            ),
            (
                Some("1"),
                all.clone()
                    .into_iter()
                    .filter(|(k, _)| [&lou1uol, &o1a].contains(&k))
                    .collect(),
            ),
            (
                Some("a$"),
                all.clone()
                    .into_iter()
                    .filter(|(k, _)| [&obaaa, &lama, &o1a].contains(&k))
                    .collect(),
            ),
            (None, all.clone()),
        ] {
            let bed = TestBed::default();
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
                        0,
                    ),
                    Action::new(
                        vec!["sh", "-c", &format!("echo a2 <az> >> {}", &bed.out_file)],
                        Some("10s"),
                        false,
                        "test",
                        "test",
                        "a2",
                        &bed.az_patterns,
                        0,
                    ),
                ],
                vec!["test <az>"],
                Some(3),
                Some("6s"),
                "test",
                "test",
                dup,
                &bed.az_patterns,
            );

            let db = TempDatabase::from_loaded_db(HashMap::from([(
                filter_triggers_db_name(filter),
                HashMap::from([
                    (
                        obaaa.clone().into(),
                        json!({now1s.as_nanos().to_string(): 1}),
                    ),
                    (
                        lama.clone().into(),
                        json!({now2s.as_nanos().to_string(): 1}),
                    ),
                    (
                        lou1uol.clone().into(),
                        json!({now3s.as_nanos().to_string(): 1}),
                    ),
                    (
                        alulu.clone().into(),
                        json!({now4s.as_nanos().to_string(): 1}),
                    ),
                    (o1a.clone().into(), json!({now5s.as_nanos().to_string(): 1})),
                ]),
            )]))
            .await;
            let bed = bed.part2(filter, now, Some(db)).await;

            // Wait for startup actions to complete
            sleep(Duration::from_millis(50)).await;
            // Then reset them
            bed.reset_out_file().await;

            dbg!(bed.manager.state.lock().await.triggers.tree());
            dbg!(read_to_string(&bed.out_file).unwrap());

            let patterns = match query {
                Some(q) => &BTreeMap::from([(
                    bed.az_patterns.iter().next().unwrap().1.clone(),
                    Regex::new(q).unwrap(),
                )]),
                None => &BTreeMap::default(),
            };

            let filtered_map = filtered
                .iter()
                .map(|(k, v)| {
                    (
                        k[0].clone(),
                        PatternStatus {
                            actions: v
                                .iter()
                                .map(|v| {
                                    // We add 10sec because the action has after: 10s
                                    ("a2".into(), vec![v.add(Duration::from_secs(10)).pretty()])
                                })
                                .collect(),
                            ..Default::default()
                        },
                    )
                })
                .collect();

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.triggers.keys().collect::<BTreeSet<_>>(),
                    all.keys().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Show, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.triggers.keys().collect::<BTreeSet<_>>(),
                    all.keys().collect(),
                );
            }

            assert_eq!(
                &bed.manager.handle_order(patterns, Order::Flush, now).await,
                &filtered_map,
            );

            {
                let state = bed.manager.state.lock().await;
                assert_eq!(
                    state.triggers.keys().collect::<BTreeSet<_>>(),
                    all.clone()
                        .into_keys()
                        .collect::<BTreeSet<_>>()
                        .difference(&filtered.into_keys().collect())
                        .collect(),
                );
            }
        }
    }
}
