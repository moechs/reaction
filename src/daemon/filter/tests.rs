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
    tests::TempDb,
};

#[allow(clippy::type_complexity)]
fn open_trees(
    db: &TempDb,
    filter: &Filter,
) -> (
    Tree<Match, BTreeSet<Time>>,
    Tree<Time, Match>,
    Tree<Match, BTreeMap<Time, usize>>,
) {
    (
        db.open_filter_matches_tree(filter).unwrap(),
        db.open_filter_ordered_times_tree(filter).unwrap(),
        db.open_filter_triggers_tree(filter).unwrap(),
    )
}

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

    fn part2(self, filter: &'static Filter, now: Time, db: Option<TempDb>) -> TestBed2 {
        let db = db.unwrap_or_default();
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
                filter,
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

    let bed = bed.part2(filter, Local::now(), None);

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

    let bed = bed.part2(filter, Local::now(), None);
    let now = bed.now;

    // No match
    assert!(!bed.manager.handle_line("test 131", now));
    bed.assert_empty_trees();

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

    let bed = bed.part2(filter, Local::now(), None);
    let now = bed.now;

    // No match
    assert!(!bed.manager.handle_line("test 131", now));
    bed.assert_empty_trees();

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

    let db = TempDb::default();
    let (matches, ordered_times, _) = open_trees(&db, filter);

    // Pre-add match
    let now = Local::now();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::seconds(1);

    matches.insert(&one, &BTreeSet::from([now1s])).unwrap();
    ordered_times.insert(&now1s, &one).unwrap();

    // Finish setup
    let bed = bed.part2(filter, now, Some(db));

    assert_eq!(
        bed.matches.as_map(),
        BTreeMap::from([(one.clone(), BTreeSet::from([now1s]))]),
        "the match previously added to matches"
    );
    assert_eq!(
        bed.ordered_times.as_map(),
        BTreeMap::from([(now1s, one.clone())]),
        "the match previously added to matches"
    );
    assert!(bed.triggers.iter().next().is_none(), "triggers stay empty");

    // match
    assert!(bed.manager.handle_line("test one", now));
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

    let db = TempDb::default();
    let (matches, ordered_times, _) = open_trees(&db, filter);

    // Pre-add match
    let now = Local::now();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::milliseconds(1001);

    matches.insert(&one, &BTreeSet::from([now1s])).unwrap();
    ordered_times.insert(&now1s, &one).unwrap();

    // Finish setup
    let bed = bed.part2(filter, now, Some(db));
    bed.assert_empty_trees();
}
