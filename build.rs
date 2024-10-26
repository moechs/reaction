use std::{
    env::var_os,
    io::{self, ErrorKind},
    path::Path,
    process,
};

use clap_complete::shells;

// SubCommand defined here
include!("src/cli.rs");

fn compile_helper(name: &str, out_dir: &Path) -> io::Result<()> {
    process::Command::new("gcc")
        .args([
            &format!("helpers_c/{name}.c"),
            "-o",
            out_dir.join(name).to_str().expect("could not join path"),
        ])
        .spawn()?;
    Ok(())
}

fn main() -> io::Result<()> {
    if var_os("PROFILE").ok_or(ErrorKind::NotFound)? == "release" {
        let out_dir = PathBuf::from(var_os("OUT_DIR").ok_or(ErrorKind::NotFound)?).join("../../..");

        // Compile C helpers
        compile_helper("ip46tables", &out_dir)?;
        compile_helper("nft46", &out_dir)?;

        // Build CLI
        let cli = clap::Command::new("reaction");
        let cli = SubCommand::augment_subcommands(cli);
        // We have to manually add metadata because it is lost: only subcommands are appended
        let cli = cli.about("Scan logs and take action").long_about(
"A daemon that scans program outputs for repeated patterns, and takes action.

Aims at being more versatile and flexible than fail2ban, while being faster and having simpler configuration.

See usage examples, service configurations and good practices on the wiki: https://reaction.ppom.me");

        // Generate completions
        clap_complete::generate_to(shells::Bash, &mut cli.clone(), "reaction", out_dir.clone())?;
        clap_complete::generate_to(shells::Fish, &mut cli.clone(), "reaction", out_dir.clone())?;
        clap_complete::generate_to(shells::Zsh, &mut cli.clone(), "reaction", out_dir.clone())?;

        // Generate manpages
        clap_mangen::generate_to(cli, out_dir.clone())?;
    }

    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=src/utils/cli.rs");
    println!("cargo::rerun-if-changed=helpers_c/ip46tables.c");
    println!("cargo::rerun-if-changed=helpers_c/nft46.c");

    Ok(())
}
