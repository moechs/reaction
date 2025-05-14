use std::{collections::{BTreeMap, BTreeSet}, fs::read_to_string, sync::Arc, time::Duration};

use chrono::{Local, TimeDelta};
use fjall::TransactionalKeyspace;
use tempfile::TempPath;
use tokio::sync::Semaphore;

use super::FilterManager;
use crate::{
    concepts::{Action, Filter, Match, Pattern, Patterns, Time},
    daemon::{filter::React, shutdown::ShutdownController, KeyspaceExt, MatchTime, Tree},
    tests::TempKeyspace,
};

macro_rules! is_empty {
    ($db:expr, $env:expr) => {{
        let rtxn = $env.read_tx();
        let ret = $db.iter(&rtxn).next().is_none();
        ret
    }};
}

macro_rules! as_map {
    ($db:expr, $env:expr) => {{
        let rtxn = $env.read_tx();
        let ret = $db
            .iter(&rtxn)
            .collect::<BTreeMap<_, _>>();
        ret
    }};
}

#[allow(clippy::type_complexity)]
fn open_trees(
    env: &TransactionalKeyspace,
    filter: &Filter,
) -> (
    Tree<Match, BTreeSet<Time>>,
    Tree<Time, Match>,
    Tree<MatchTime, usize>,
) {
    (
        env.open_filter_matches_db(filter).unwrap(),
        env.open_filter_ordered_times_db(filter).unwrap(),
        env.open_filter_triggers_db(filter).unwrap(),
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

    fn part2(self, filter: &'static Filter, now: Time, env: Option<TempKeyspace>) -> TestBed2 {
        let env = env.unwrap_or_default();
        let controller = ShutdownController::new();
        let semaphore = Arc::new(Semaphore::new(1));
        TestBed2 {
            _out_path: self._out_path,
            out_file: self.out_file,
            now,
            matches: env.open_filter_matches_db(filter).unwrap(),
            ordered_times: env.open_filter_ordered_times_db(filter).unwrap(),
            triggers: env.open_filter_triggers_db(filter).unwrap(),
            manager: FilterManager::new(
                filter,
                Some(semaphore.clone()),
                controller.token(),
                &env,
                now,
            )
            .unwrap(),
            // controller,
            semaphore,
            env,
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
    pub env: TempKeyspace,
    pub matches: Tree<Match, BTreeSet<Time>>,
    pub ordered_times: Tree<Time, Match>,
    pub triggers: Tree<MatchTime, usize>,
    pub manager: FilterManager,
}

impl TestBed2 {
    fn assert_empty_trees(&self) {
        assert!(is_empty!(&self.matches, &self.env), "matches must be empty");
        assert!(
            is_empty!(&self.ordered_times, &self.env),
            "ordered_times must be empty"
        );
        assert!(
            is_empty!(&self.triggers, &self.env),
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
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // First match
    assert_eq!(bed.manager.handle_line("test one", now), React::Match);
    let one = vec!["one".to_string()];
    assert_eq!(
        as_map!(&bed.matches, &bed.env),
        BTreeMap::from([(one.clone(), BTreeSet::from([now]))]),
        "the match has been added to matches"
    );
    assert_eq!(
        as_map!(&bed.ordered_times, &bed.env),
        BTreeMap::from([(now, one.clone())]),
        "the match has been added to ordered_times"
    );
    assert!(
        is_empty!(&bed.triggers, &bed.env),
        "triggers is still empty"
    );

    // Second match
    assert_eq!(bed.manager.handle_line("test one", now1s), React::Match);
    assert_eq!(
        as_map!(&bed.matches, &bed.env),
        BTreeMap::from([(one.clone(), BTreeSet::from([now, now1s]))]),
        "a second match is present in matches"
    );
    assert_eq!(
        as_map!(&bed.ordered_times, &bed.env),
        BTreeMap::from([(now, one.clone()), (now1s, one.clone())]),
        "a second match is present in ordered_times"
    );
    assert!(
        is_empty!(&bed.triggers, &bed.env),
        "triggers is still empty"
    );

    // Third match, exec
    let _block = bed.semaphore.acquire().await.unwrap();
    assert_eq!(bed.manager.handle_line("test one", now2s), React::Exec);
    dbg!(as_map!(&bed.matches, &bed.env));
    assert!(
        is_empty!(&bed.matches, &bed.env),
        "matches are emptied after trigger"
    );
    assert!(
        is_empty!(&bed.ordered_times, &bed.env),
        "ordered_times are emptied after trigger"
    );
    assert_eq!(
        as_map!(&bed.triggers, &bed.env),
        BTreeMap::from([(
            MatchTime {
                m: one.clone(),
                t: now2s
            },
            1
        )]),
        "triggers now contain the triggered match with 1 action left" // 1 and not 2 because the decrement_trigger() doesn't wait for the semaphore
    );
    drop(_block);

    // Now the first action executes
    tokio::time::sleep(Duration::from_millis(40)).await;
    // Check first action
    assert_eq!(
        as_map!(&bed.triggers, &bed.env),
        BTreeMap::from([(
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
        is_empty!(&bed.triggers, &bed.env),
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
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // match
    assert_eq!(bed.manager.handle_line("test one", now), React::Exec);
    assert!(is_empty!(&bed.matches, &bed.env), "matches stay empty");
    assert!(
        is_empty!(&bed.ordered_times, &bed.env),
        "ordered_times stay empty"
    );
    assert!(is_empty!(&bed.triggers, &bed.env), "triggers stay empty");

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
    assert_eq!(bed.manager.handle_line("test 131", now), React::NoMatch);
    bed.assert_empty_trees();

    // Match
    let one = vec!["one".to_string()];
    assert_eq!(bed.manager.handle_line("test one", now), React::Exec);
    assert!(is_empty!(&bed.matches, &bed.env), "matches stay empty");
    assert!(
        is_empty!(&bed.ordered_times, &bed.env),
        "ordered_times stay empty"
    );
    assert_eq!(
        as_map!(&bed.triggers, &bed.env),
        BTreeMap::from([(
            MatchTime {
                m: one.clone(),
                t: now,
            },
            1
        )]),
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
        is_empty!(&bed.triggers, &bed.env),
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

    let db = TempKeyspace::default();
    let (matches, ordered_times, _) = open_trees(&db, filter);

    // Pre-add match
    let now = Local::now();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::seconds(1);

    let mut wtxn = db.write_tx();
    matches.insert(&mut wtxn, &one, &BTreeSet::from([now1s]));
    ordered_times.insert(&mut wtxn, &now1s, &one);
    wtxn.commit().unwrap();

    // Finish setup
    let bed = bed.part2(filter, now, Some(db));

    assert_eq!(
        as_map!(&bed.matches, &bed.env),
        BTreeMap::from([(one.clone(), BTreeSet::from([now1s]))]),
        "the match previously added to matches"
    );
    assert_eq!(
        as_map!(&bed.ordered_times, &bed.env),
        BTreeMap::from([(now1s, one.clone())]),
        "the match previously added to matches"
    );
    assert!(is_empty!(&bed.triggers, &bed.env), "triggers stay empty");

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

    let db = TempKeyspace::default();
    let (matches, ordered_times, _) = open_trees(&db, filter);

    // Pre-add match
    let now = Local::now();
    let one = vec!["one".to_string()];
    let now1s = now - TimeDelta::milliseconds(1001);

    let mut wtxn = db.write_tx();
    matches.insert(&mut wtxn, &one, &BTreeSet::from([now1s]));
    ordered_times.insert(&mut wtxn, &now1s, &one);
    wtxn.commit().unwrap();

    // Finish setup
    let bed = bed.part2(filter, now, Some(db));
    bed.assert_empty_trees();
}
