#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages (ps: with ps; [ requests ])" -p debian-devscripts git minisign cargo-cross
import base64
import http.client
import json
import os
import subprocess
import sys
import tempfile

def quit_if(cmd):
    if cmd.returncode != 0:
        print(f"{' '.join(cmd.args)} failed with exit code {cmd.returncode}")
        sys.exit(1)

def add_path(files, architecture):
    return [
        (f"./target/{architecture}/release/{file[0]}", file[1], file[2], file[3])
        for file in files
    ]

def main():
    # Git tag
    cmd = subprocess.run(["git", "tag", "--sort=v:refname"], capture_output=True, text=True)
    quit_if(cmd)
    tag = ""
    try:
        tag = cmd.stdout.strip().split("\n")[-1]
    except:
        pass
    if tag == "":
        print("could not retrieve last git tag.")
        sys.exit(1)

    # Ask user
    if input(f"We will create a release for tag {tag}. Do you want to continue? (y/n) ") != "y":
        print("exiting.")
        sys.exit(1)

    # Git push
    cmd = subprocess.run(["git", "push", "--tags"])
    quit_if(cmd)

    cmd = subprocess.run(["rbw", "get", "minisign"], capture_output=True, text=True)
    quit_if(cmd)
    minisign_password = cmd.stdout

    all_files = []

    architectures = [
        ("x86_64-unknown-linux-gnu", "amd64"),
        # "x86_64-unknown-openbsd", # not supported by cross
        ("armv7-unknown-linux-gnueabihf", "arm"),
    ]

    for archs in architectures:
        go_arch = archs[1]
        architecture = archs[0]

        # Install toolchain
        # cmd = subprocess.run([
        #     "rustup", "toolchain", "install", "stable",
        #     "-t", architecture,
        #     "--profile", "minimal"])
        # quit_if(cmd)

        # Build
        cmd = subprocess.run([
            "cross", "build", "--release", "--target", architecture
        ])
        quit_if(cmd)

        # Build rust import db
        os.chdir("./import-rust-db")
        cmd = subprocess.run([
            "cross", "build", "--release", "--target", architecture
        ])
        quit_if(cmd)
        with open(f"./target/{architecture}/release/import-rust-db", "rb") as file:
            rust_contents = base64.standard_b64encode(file.read())
        os.chdir("..")

        # Build go export db
        os.chdir("./export-go-db")
        cmd = subprocess.run(
            ["go", "build", "export-go-db.go"],
            env=os.environ.update({"GOARCH": go_arch})
        )
        quit_if(cmd)
        with open("./export-go-db", "rb") as file:
            go_contents = base64.standard_b64encode(file.read())
        os.chdir("..")

        # Build glue script
        contents = """
function main() {
    set -eu -o pipefail
    DIR="$(mktemp -d)"
    echo "$GOBIN" | base64 -d > "$DIR/gobin"
    echo "$RUBIN" | base64 -d > "$DIR/rubin"
    chmod +x "$DIR/gobin" "$DIR/rubin"
    "$DIR/gobin"
    "$DIR/rubin"
    rm "$DIR/gobin" "$DIR/rubin"
    rmdir "$DIR"
}

GOBIN=""".encode() + go_contents + """
RUBIN=""".encode() + rust_contents + """

main
""".encode()
        del go_contents
        del rust_contents

        with open(f"./target/{architecture}/release/migrate_reaction_db", "bw+") as file:
            file.write(contents)

        del contents

        # # Build
        # cmd = subprocess.run([
        #     "docker", "run", "-it", "--rm",
        #     "-e", "HOME=/tmp/",
        #     "-v", f"{os.getcwd()}:/tmp/code",
        #     "-w", "/tmp/code",
        #     "-u", str(os.getuid()),
        #     "rust",
        #     "make", f"reaction_{tag}-1_amd64.deb", "reaction", "ip46tables", "nft46"
        # ])
        # quit_if(cmd)

        # File lists
        binary_files = [
            ("reaction", architecture, f"reaction ({architecture})", "package"),
            ("migrate_reaction_db", architecture, f"db migration script ({architecture})", "package"),
            ("nft46", architecture, f"nft46 ({architecture})", "package"),
            ("ip46tables", architecture, f"ip46tables ({architecture})", "package"),
            # (f"reaction_{tag}-1_amd64.deb", architecture, f"reaction.deb ({architecture})", "package")
        ]

        sig_files = [
            (f"{file[0]}.minisig", architecture, f"{file[0]}.minisig ({architecture})", "other")
            for file
            in binary_files
        ]

        binary_files = add_path(binary_files, architecture)
        sig_files = add_path(sig_files, architecture)

        # Sign
        cmd = subprocess.run(
            ["minisign", "-Sm"]
            + [ file[0] for file in binary_files ],
            text=True,
            input=minisign_password
        )
        quit_if(cmd)

        # Create directory
        cmd = subprocess.run([
            "ssh", "akesi",
            "-J", "pica01",
            "mkdir", "-p", f"/var/www/static/reaction/releases/{tag}/{architecture}/"
        ])
        quit_if(cmd)

        # Push
        cmd = subprocess.run([
            "rsync",
            "-avze", "ssh -J pica01", ]
            + [ file[0] for file in binary_files + sig_files ]
            + [ f"akesi:/var/www/static/reaction/releases/{tag}/{architecture}/"
        ])
        quit_if(cmd)

        all_files.extend(binary_files)
        all_files.extend(sig_files)

        # Copy only one time the text files, which are architecture-independant
        if archs == architectures[-1]:
            text_files = [
                ("reaction.bash", "", "bash completion file", "other"),
                ("reaction.fish", "", "fish completion file", "other"),
                ("_reaction", "", "zsh completion file", "other"),
                ("reaction.1", False, False, False),
                ("reaction-flush.1", False, False, False),
                ("reaction-show.1", False, False, False),
                ("reaction-start.1", False, False, False),
                ("reaction-test-regex.1", False, False, False),
            ]
            text_files = add_path(text_files, architecture)
            all_files.extend(text_files)

            cmd = subprocess.run([
                "rsync",
                "-avze", "ssh -J pica01" ]
                + [ file[0] for file in text_files ]
                + [ f"akesi:/var/www/static/reaction/releases/{tag}/"
            ])
            quit_if(cmd)

    # Release

    cmd = subprocess.run(["rbw", "get", "framagit.org", "token"], capture_output=True, text=True)
    quit_if(cmd)
    token = cmd.stdout.strip()
    if token == "":
        print("Could not retrieve token")
        sys.exit(1)

    description = f"""
## Changes

## Direct download

```bash
wget https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/nft46 \\
     https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/reaction \\
     https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/ip46tables \\
     https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/nft46.minisig \\
     https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/reaction.minisig \\
     https://static.ppom.me/reaction/releases/{architectures[0][0]}/{tag}/ip46tables.minisig
for i in nft46 ip46tables reaction
do
    minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX -m $i && rm $i.minisig
done
```

<!--
## Debian Installation

```bash
wget https://static.ppom.me/reaction/releases/{tag}/reaction_{tag}-1_amd64.deb \\
     https://static.ppom.me/reaction/releases/{tag}/reaction_{tag}-1_amd64.deb.minisig \\
    && minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX \\
                -m reaction_{tag}-1_amd64.deb \\
    && rm reaction_{tag}-1_amd64.deb.minisig \\
    && apt install ./reaction_{tag}-1_amd64.deb
-->
```
"""

    # Make user edit the description
    tmpdir = tempfile.TemporaryDirectory()
    desc_path = tmpdir.name + "/description.md"
    with open(desc_path, "w+") as desc_file:
        desc_file.write(description)
    cmd = subprocess.run(["vi", desc_path])
    quit_if(cmd)

    with open(desc_path) as desc_file:
        description = desc_file.read().strip()

    if description == "":
        print()
        print("User deleted emptied description, exiting.")
        sys.exit(1)

    # Construct JSON payload
    files = [ file for file in all_files if file[2] != False ]
    data = {
        "tag_name": tag,
        "description": description,
        "assets": {
            "links": [
                {
                    "url": "https://" + f"static.ppom.me/reaction/releases/{tag}/{file[1]}/{os.path.basename(file[0])}".replace("//", "/"),
                    "name": file[2],
                    "link_type": file[3],
                }
                for file
                in files
            ]
        }
    }

    body = json.dumps(data)

    # Send POST request
    headers = {
        "Host": "framagit.org",
        "Content-Type": "application/json",
        "PRIVATE-TOKEN": token,
    }

    conn = http.client.HTTPSConnection("framagit.org")
    conn.request("POST", "/api/v4/projects/90566/releases", body=body, headers=headers)
    response = conn.getresponse()

    if response.status != 200:
        print(f"sending message failed: status: {response.status}, reason: {response.reason}")
        sys.exit(1)

main()
