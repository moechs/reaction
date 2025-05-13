use std::{collections::BTreeSet, fs::create_dir, io::ErrorKind};

use heed::{
    byteorder::LittleEndian,
    types::{DecodeIgnore, SerdeBincode, Str, U64},
    Database, DatabaseFlags, Env, EnvFlags, Result,
};
use serde::{Deserialize, Serialize};

use crate::concepts::{Config, Filter, Match, Time};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MatchTime {
    pub m: Match,
    pub t: Time,
}

pub fn open_db(config: &Config) -> std::result::Result<Env, String> {
    let lmdb_dir = format!("{}/lmdb", config.state_directory());

    match create_dir(&lmdb_dir) {
        Ok(_) => Ok(()),
        Err(err) => match err.kind() {
            ErrorKind::AlreadyExists => Ok(()),
            _ => Err(format!(
                "while creating database directory {}: {}",
                config.state_directory(),
                err
            )),
        },
    }?;

    let max_dbs = config
        .streams()
        .values()
        .map(|stream| stream.filters().len())
        // We have 3 databases per filter + 1 unnamed db
        .fold(1, |acc, elt| acc + 3 * elt);

    // We have no choice but to use unsafe, as the library exposes unsafe code
    #[allow(unsafe_code)]
    let env = unsafe {
        heed::EnvOpenOptions::new()
            // safety: if you have more than 1.4 billion filters,
            // you searched for this overflow :smirk:
            .max_dbs(max_dbs as u32)
            .map_size(100000 * 4096)
            .flags(EnvFlags::NO_SYNC)
            .open(lmdb_dir)
    }
    .map_err(|err| {
        format!(
            "while opening database at {}: {}",
            config.state_directory(),
            err
        )
    })?;

    // FIXME not cleaning databases because it would make us open too much dbs,
    // as we can't delete them nor close them for now.
    // See FIXME below for more information.
    // env.cleanup_unused_dbs(config).map_err(|err| {
    //     format!(
    //         "while cleaning database on startup at {}: {}",
    //         config.state_directory(),
    //         err
    //     )
    // })?;

    Ok(env)
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

    #[allow(dead_code)] // See FIXME
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
