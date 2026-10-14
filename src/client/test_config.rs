use std::{error::Error, path::PathBuf};

use crate::{cli::Format, concepts::Config};

pub fn test_config(
    config_path: Option<PathBuf>,
    format: Format,
    verbose: bool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (mut cfg, cfg_files, config_path) = Config::from_path_raw(&config_path)?;
    if verbose {
        if config_path.is_dir() {
            println!(
                "Loaded the configuration from the following files in the directory {} in this order:",
                config_path.display()
            );
            println!("{}\n", cfg_files.join("\n"));
        } else {
            println!(
                "Loaded the configuration from the file {}",
                config_path.display()
            );
        }
    }

    // first serialize the raw config (before regexes are transformed and their original version
    // discarded)
    let cfg_str = match format {
        Format::JSON => serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string()),
        Format::YAML => serde_saphyr::to_string(&cfg).map_err(|e| e.to_string()),
    }
    .map_err(|e| format!("Error serializing back the configuration: {e}"))?;

    // then try to finalize the configuration: will raise an error on an invalid config
    cfg.setup()
        .map_err(|e| format!("Configuration file {}: {}", config_path.display(), e))?;

    // only print the serialized config if everyting went well
    println!("{cfg_str}");

    Ok(())
}
