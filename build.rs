use std::{
    env::{var, var_os},
    io::{self, ErrorKind},
    path::Path,
    process,
};

use clap_complete::shells;

// SubCommand defined here
include!("src/cli.rs");

fn cc() -> String {
    // TARGET looks like aarch64-unknown-linux-musl
    let cc = match var("TARGET") {
        Ok(target) => {
            // We're looking for an environment variable looking like
            // CC_aarch64_unknown_linux_musl
            let target = target.replace("-", "_");
            var(format!("CC_{}", target.replace("-", "_"))).ok()
        }
        Err(_) => None,
    };
    match cc {
        Some(cc) => Some(cc),
        // Else we're looking for CC environment variable
        None => var("CC").ok(),
    }
    // Else we use `cc`
    .unwrap_or("cc".into())
}

fn compile_helper(cc: &str, name: &str, out_dir: &Path) -> io::Result<()> {
    let mut args = vec![
        format!("helpers_c/{name}.c"),
        "-o".into(),
        out_dir
            .join(name)
            .to_str()
            .expect("could not join path")
            .to_owned(),
    ];
    // We can build static executables in cross environment
    if cc.ends_with("-gcc") {
        args.push("-static".into());
    }
    process::Command::new(cc).args(args).spawn()?;
    Ok(())
}

fn main() -> io::Result<()> {
    if var_os("PROFILE").ok_or(ErrorKind::NotFound)? == "release" {
        let out_dir = PathBuf::from(var_os("OUT_DIR").ok_or(ErrorKind::NotFound)?).join("../../..");

        // Compile C helpers
        let cc = cc();
        println!("CC is: {}", cc);
        compile_helper(&cc, "ip46tables", &out_dir)?;
        compile_helper(&cc, "nft46", &out_dir)?;

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
    println!("cargo::rerun-if-changed=src/cli.rs");
    println!("cargo::rerun-if-changed=helpers_c/ip46tables.c");
    println!("cargo::rerun-if-changed=helpers_c/nft46.c");

    Ok(())
}
