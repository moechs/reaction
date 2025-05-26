use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::read_to_string,
    sync::Arc,
    time::Duration,
};

use chrono::{Local, TimeDelta, TimeZone};
use tempfile::TempPath;
use tokio::sync::Semaphore;

use super::{filter_ordered_times_db_name, FilterManager, React};
use crate::{
    concepts::{Action, Filter, MatchTime, Pattern, Patterns, Time},
    daemon::shutdown::ShutdownController,
    tests::TempDatabase,
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

    async fn part2(self, filter: &'static Filter, now: Time, db: Option<TempDatabase>) -> TestBed2 {
        let mut db = match db {
            Some(db) => db,
            None => TempDatabase::default().await,
        };
        let controller = ShutdownController::new();
        let semaphore = Arc::new(Semaphore::new(1));
        TestBed2 {
            _out_path: self._out_path,
            out_file: self.out_file,
            now,
            manager: FilterManager::new(
                filter,
                Some(semaphore.clone()),
                controller.token(),
                &mut db,
                now,
            )
            .unwrap(),
            semaphore,
        }
    }
}

struct TestBed2 {
    pub _out_path: TempPath,
    pub out_file: String,
    pub semaphore: Arc<Semaphore>,
    pub now: Time,
    pub manager: FilterManager,
}

impl TestBed2 {
    fn assert_empty_trees(&self) {
        let state = self.manager.state.lock().unwrap();
        assert!(state.matches.is_empty(), "matches must be empty");
        assert!(
            state.ordered_times.is_empty(),
            "ordered_times must be empty"
        );
        assert!(state.triggers.is_empty(), "triggers must be empty");
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

    let bed = bed.part2(filter, Local::now(), None).await;

    let now = bed.now;
    let now1s = bed.now + TimeDelta::seconds(1);
    let now2s = bed.now + TimeDelta::seconds(2);

    // No match
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // First match
    let one = vec!["one".to_string()];
    assert_eq!(bed.manager.handle_line("test one", now), React::Match);
    {
        let state = bed.manager.state.lock().unwrap();
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
    assert_eq!(bed.manager.handle_line("test one", now1s), React::Match);
    {
        let state = bed.manager.state.lock().unwrap();
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
    assert_eq!(bed.manager.handle_line("test one", now2s), React::Exec);
    {
        let state = bed.manager.state.lock().unwrap();
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
            &BTreeMap::from([(
                MatchTime {
                    m: one.clone(),
                    t: now2s
                },
                1
            )]),
            // 1 and not 2 because the decrement_trigger() doesn't wait for the semaphore
            "triggers now contain the triggered match with 1 action left"
        );
    }
    drop(_block);

    // Now the first action executes
    tokio::time::sleep(Duration::from_millis(40)).await;
    // Check first action
    assert_eq!(
        bed.manager.state.lock().unwrap().triggers.tree(),
        &BTreeMap::from([(
            MatchTime {
                m: one.clone(),
                t: now2s
            },
            1
        )]),
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
        bed.manager.state.lock().unwrap().triggers.is_empty(),
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

    let bed = bed.part2(filter, Local::now(), None).await;
    let now = bed.now;

    // No match
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // match
    assert_eq!(bed.manager.handle_line("test one", now), React::Exec);
    bed.assert_empty_trees();

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

    let bed = bed.part2(filter, Local::now(), None).await;
    let now = bed.now;

    // No match
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // Match
    let one = vec!["one".to_string()];
    assert_eq!(bed.manager.handle_line("test one", now), React::Exec);
    {
        let state = bed.manager.state.lock().unwrap();
        assert!(state.matches.is_empty(), "matches stay empty");
        assert!(state.ordered_times.is_empty(), "ordered_times stay empty");
        assert_eq!(
            state.triggers.tree(),
            &BTreeMap::from([(
                MatchTime {
                    m: one.clone(),
                    t: now,
                },
                1
            )]),
            "triggers still contain the triggered match with 1 action left"
        );
    }
    assert_eq!(
        "",
        &read_to_string(&bed.out_file).unwrap(),
        "the output file is empty"
    );

    // The action executes
    tokio::time::sleep(Duration::from_millis(140)).await;
    assert!(
        bed.manager.state.lock().unwrap().triggers.is_empty(),
        "triggers are empty again"
    );
    assert_eq!(
        "a1 one\n",
        &read_to_string(&bed.out_file).unwrap(),
        "the output file contains the result of the action"
    );

    bed.assert_empty_trees();
}

#[tokio::test]
async fn one_db_match_one_runtime_match_one_action() {
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
        Some(2),
        Some("2s"),
        "test",
        "test",
        &bed.az_patterns,
    );

    let mut db = TempDatabase::default().await;

    // Pre-add match
    // Truncate to millisecond precision
    let now = Local
        .timestamp_millis_opt(Local::now().timestamp_millis())
        .unwrap();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::seconds(1);

    db.set_loaded_db(HashMap::from([(
        filter_ordered_times_db_name(&filter),
        HashMap::from([(now1s.timestamp_millis().into(), one.clone().into())]),
    )]));

    // Finish setup
    let bed = bed.part2(filter, now, Some(db)).await;

    {
        let state = bed.manager.state.lock().unwrap();
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
    assert_eq!(bed.manager.handle_line("test one", now), React::Exec);
    bed.assert_empty_trees();
    // the action executes
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        "a1 one\n",
        &read_to_string(&bed.out_file).unwrap(),
        "the output file contains the result of the action"
    );
}

#[tokio::test]
async fn one_outdated_db_match() {
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
        Some(2),
        Some("1s"),
        "test",
        "test",
        &bed.az_patterns,
    );

    let mut db = TempDatabase::default().await;

    // Pre-add match
    let now = Local::now();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::milliseconds(1001);

    db.set_loaded_db(HashMap::from([(
        filter_ordered_times_db_name(&filter),
        HashMap::from([(now1s.timestamp_millis().into(), one.clone().into())]),
    )]));

    // Finish setup
    let bed = bed.part2(filter, now, Some(db)).await;
    bed.assert_empty_trees();
}
