use std::collections::{BTreeMap, BTreeSet};

use heed::{
    types::{DecodeIgnore, Str},
    Database, Result,
};

use crate::concepts::{Config, Filter, Match, Time};

/// This trait permits to manage in a single place what are the names of [`heed::Database`]s in
/// reaction. It streamlines [`heed::Database`]s opening so that we can reliably open the same
/// Databases in multiple places.
/// It also permits to manage the cleanup of unused databases.
pub trait EnvExt {
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<Database<Match, BTreeSet<Time>>>;
    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<Database<Time, Match>>;
    fn open_filter_triggers_tree(
        &self,
        filter: &Filter,
    ) -> Result<Database<Match, BTreeMap<Time, usize>>>;

    fn cleanup_unused_trees(&self, config: &Config) -> Result<()>;
}

fn filter_matches_tree_name(filter: &Filter) -> String {
    format!("filter_matches_{}.{}", filter.stream_name(), filter.name())
}

fn filter_ordered_times_tree_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

fn filter_triggers_tree_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

impl EnvExt for heed::Env {
    fn open_filter_matches_tree(&self, filter: &Filter) -> Result<Database<Match, BTreeSet<Time>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database::<Match, BTreeSet<Time>>(
            &mut wtxn,
            Some(&filter_matches_tree_name(filter)),
        );
        wtxn.commit()?;
        res
    }

    fn open_filter_ordered_times_tree(&self, filter: &Filter) -> Result<Database<Time, Match>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database::<Time, Match>(
            &mut wtxn,
            Some(&filter_ordered_times_tree_name(filter)),
        );
        wtxn.commit()?;
        res
    }

    fn open_filter_triggers_tree(
        &self,
        filter: &Filter,
    ) -> Result<Database<Match, BTreeMap<Time, usize>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database::<Match, BTreeMap<Time, usize>>(
            &mut wtxn,
            Some(&filter_triggers_tree_name(filter)),
        );
        wtxn.commit()?;
        res
    }

    fn cleanup_unused_trees(&self, config: &Config) -> Result<()> {
        let valid_tree_names: BTreeSet<_> = config
            .streams()
            .values()
            // for each filter
            .flat_map(|stream| stream.filters().values())
            .flat_map(|filter| {
                [
                    filter_matches_tree_name(filter),
                    filter_ordered_times_tree_name(filter),
                    filter_triggers_tree_name(filter),
                ]
            })
            // plus sled's default map
            .chain(std::iter::once("__sled__default".into()))
            // convert as IVec which is sled's binary type
            // .map(|string| sled::IVec::from(string.as_bytes()))
            .collect();

        let rtxn = self.read_txn()?;
        let unnamed: Database<Str, DecodeIgnore> = self
            .open_database(&rtxn, None)?
            .expect("the unnamed database always exists");

        // Remove trees that are not in the list of valid trees
        for outdated_tree in unnamed
            .iter(&rtxn)?
            .filter(|key| key.is_ok())
            .map(|key| key.unwrap().0)
            .filter(|tree_name| !valid_tree_names.contains(*tree_name))
        {
            self.remove_database(outdated_tree)
                .expect("Fatal error while cleaning DB on startup");
        }

        Ok(())
    }
}
