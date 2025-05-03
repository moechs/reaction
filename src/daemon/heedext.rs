use std::collections::BTreeSet;

use heed::{
    byteorder::LittleEndian, types::{DecodeIgnore, SerdeBincode, Str, U64}, Database, DatabaseFlags, Result
};
use serde::{Deserialize, Serialize};

use crate::concepts::{Config, Filter, Match, Time};

#[derive(Serialize, Deserialize)]
pub struct MatchTime {
    pub m: Match,
    pub t: Time,
}

/// This trait permits to manage in a single place what are the names of [`heed::Database`]s in
/// reaction. It streamlines [`heed::Database`]s opening so that we can reliably open the same
/// Databases in multiple places.
/// It also permits to manage the cleanup of unused databases.
pub trait EnvExt {
    fn open_filter_matches_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Match>, SerdeBincode<Time>>>;
    fn open_filter_ordered_times_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Time>, SerdeBincode<Match>>>;
    fn open_filter_triggers_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<MatchTime>, U64<LittleEndian>>>;

    fn cleanup_unused_dbs(&self, config: &Config) -> Result<()>;
}

fn filter_matches_db_name(filter: &Filter) -> String {
    format!("filter_matches_{}.{}", filter.stream_name(), filter.name())
}

fn filter_ordered_times_db_name(filter: &Filter) -> String {
    format!(
        "filter_ordered_times_{}.{}",
        filter.stream_name(),
        filter.name()
    )
}

fn filter_triggers_db_name(filter: &Filter) -> String {
    format!("filter_triggers_{}.{}", filter.stream_name(), filter.name())
}

impl EnvExt for heed::Env {
    fn open_filter_matches_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Match>, SerdeBincode<Time>>> {
        let mut wtxn = self.write_txn()?;
        let res = self
            .database_options()
            .name(&filter_matches_db_name(filter))
            // We allow duplicates on this database
            .flags(DatabaseFlags::DUP_SORT)
            .types::<SerdeBincode<Match>, SerdeBincode<Time>>()
            .create(&mut wtxn);
        wtxn.commit()?;
        res
    }

    fn open_filter_ordered_times_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<Time>, SerdeBincode<Match>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database(&mut wtxn, Some(&filter_ordered_times_db_name(filter)));
        wtxn.commit()?;
        res
    }

    fn open_filter_triggers_db(
        &self,
        filter: &Filter,
    ) -> Result<Database<SerdeBincode<MatchTime>, U64<LittleEndian>>> {
        let mut wtxn = self.write_txn()?;
        let res = self.create_database::<SerdeBincode<MatchTime>, U64<LittleEndian>>(
            &mut wtxn,
            Some(&filter_triggers_db_name(filter)),
        );
        wtxn.commit()?;
        res
    }

    fn cleanup_unused_dbs(&self, config: &Config) -> Result<()> {
        let valid_db_names: BTreeSet<_> = config
            .streams()
            .values()
            // for each filter
            .flat_map(|stream| stream.filters().values())
            .flat_map(|filter| {
                [
                    filter_matches_db_name(filter),
                    filter_ordered_times_db_name(filter),
                    filter_triggers_db_name(filter),
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

        let mut wtxn = self.write_txn()?;
        // Remove dbs that are not in the list of valid dbs
        for outdated_db in unnamed
            .iter(&rtxn)?
            .filter(|key| key.is_ok())
            // unwrap because we only kept `Result::Ok`s
            .map(|key| key.unwrap().0)
            .filter(|db_name| !valid_db_names.contains(*db_name))
        {
            if let Some(db) =
                self.open_database::<DecodeIgnore, DecodeIgnore>(&wtxn, Some(outdated_db))?
            {
                // FIXME Waiting for this PR to land in a stable release
                // https://github.com/meilisearch/heed/pull/330
                // For now we're just clearing it
                db.clear(&mut wtxn)?;
                // db.remove(&mut write_txn)?
            }
        }

        wtxn.commit()?;
        Ok(())
    }
}
