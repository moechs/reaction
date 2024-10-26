use std::{
    collections::BTreeMap,
    error::Error,
    fs::File,
    io::{self, BufReader, BufWriter, Write},
    process::exit,
};

use bincode::Options;
use serde::{Deserialize, Serialize};
use serde_json::Deserializer;
// use serde_json::deserialize_from;

const DB: &str = "./reaction-matches";
const FLUSH: &str = "./reaction-flushes";

const NORMAL: &str = ".db";
const OLD: &str = ".db.old";
const EXPORT: &str = ".export.json";

type E = Box<dyn Error>;

fn main() {
    if let Err(err) = lil_main() {
        println!("fatal: {err}");
        exit(1);
    }
}

fn lil_main() -> Result<(), E> {
    println!("Hello from Rust!");
    println!("You're about to reimport the previously exported database.");
    println!("This will move the old database files as .old");
    println!("When the process completes, you'll be able to run reaction-v2.");
    println!(
        "If reaction-v2 runs as you wish, you'll be free to delete the .old & .export.json files"
    );
    println!("Do you want to continue? (y/n)");
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .map_err(|err| format!("could not read user input: {err}"))?;
    if buffer != "y\n" {
        return Err("user did not type `y`, exiting.".into());
    }

    std::fs::rename(format!("{DB}{NORMAL}"), format!("{DB}{OLD}"))?;
    std::fs::rename(format!("{FLUSH}{NORMAL}"), format!("{FLUSH}{OLD}"))?;

    import(format!("{DB}{EXPORT}"), format!("{DB}{NORMAL}"))?;
    import(format!("{FLUSH}{EXPORT}"), format!("{FLUSH}{NORMAL}"))?;
    Ok(())
}

fn import(json_path: &str, write_path: &str) -> Result<(), E> {
    let json_file = BufReader::new(File::open(json_path.clone())?);
    let mut write_file = BufWriter::new(File::create(write_path)?);
    let bin = bincode_options();

    // Signature writing
    if let Err(err) = write_file.write_all(DB_SIGNATURE.as_bytes()) {
        return Err(format!("Failed to write to DB: {}", err).into());
    }

    let header = collect_stream_filters(json_path)?;
    let database_header: DatabaseHeader =
        header.iter().map(|(k, v)| (v.clone(), k.clone())).collect();

    bin.serialize_into(&mut write_file, &database_header)?;

    let deserializer = Deserializer::from_reader(json_file);

    for json_entry in deserializer.into_iter::<JsonEntry>() {
        bin.serialize_into(
            &mut write_file,
            &ComputedLogEntry::from(json_entry?, &header)?,
        )?;
    }

    write_file.flush()?;
    Ok(())
}

fn collect_stream_filters(json_path: &str) -> Result<WriteHeader, E> {
    let mut header = BTreeMap::new();
    let mut count = 0;
    let json_file = BufReader::new(File::open(json_path)?);
    let deserializer = Deserializer::from_reader(json_file);

    for json_entry in deserializer.into_iter::<JsonEntry>() {
        let json_entry = json_entry?;
        let tuple = (json_entry.stream, json_entry.filter);
        if header.get(&tuple).is_none() {
            header.insert(tuple, count);
            count += 1;
        }
    }
    Ok(header)
}

#[derive(Debug, Deserialize)]
struct JsonEntry {
    time: i64,
    stream: String,
    filter: String,
    #[serde(rename = "match")]
    match_: Vec<String>,
    exec: bool,
}

// Pasted from main code

const DB_SIGNATURE: &str = "reaction-db-v01";

pub type BincodeOptions = bincode::config::WithOtherIntEncoding<
    bincode::config::DefaultOptions,
    bincode::config::VarintEncoding,
>;
pub fn bincode_options() -> BincodeOptions {
    bincode::DefaultOptions::new().with_varint_encoding()
}

type DatabaseHeader = BTreeMap<usize, (String, String)>;
type WriteHeader = BTreeMap<(String, String), usize>;

#[derive(Debug, Serialize)]
struct ComputedLogEntry {
    pub m: Vec<String>,
    pub f: usize,
    pub t: i64,
    pub exec: bool,
}

impl ComputedLogEntry {
    fn from(value: JsonEntry, header: &WriteHeader) -> Result<Self, E> {
        match header.get(&(value.stream.clone(), value.filter.clone())) {
            Some(f) => Ok(ComputedLogEntry {
                m: value.match_,
                f: *f,
                t: value.time,
                exec: value.exec,
            }),
            None => Err(format!("invalid filter: {value:?}").into()),
        }
    }
}
