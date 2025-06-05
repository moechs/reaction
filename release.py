#!/usr/bin/env nix-shell
#!nix-shell -i python3 -p "python3.withPackages (ps: with ps; [ requests ])" -p debian-devscripts git minisign cargo-cross rustup cargo-deb
import argparse
import http.client
import json
import os
import subprocess
import shutil
import sys
import tempfile


def run_command(args, **kwargs):
    print(f"\033[36mCMD: {args}\033[0m")
    cmd = subprocess.run(args, **kwargs)
    if cmd.returncode != 0:
        print(f"\033[31mCMD failed with exit code {cmd.returncode}\033[0m")
        sys.exit(1)
    return cmd


def main():
    # CLI arguments
    parser = argparse.ArgumentParser(description="create a reaction release")
    parser.add_argument(
        "-p",
        "--publish",
        action="store_true",
        help="publish a release. else build only",
    )
    args = parser.parse_args()

    root_dir = os.getcwd()

    # Git tag
    cmd = run_command(
        ["git", "tag", "--sort=-creatordate"], capture_output=True, text=True
    )
    tag = ""
    try:
        tag = cmd.stdout.strip().split("\n")[0]
    except Exception:
        pass
    if tag == "":
        print("could not retrieve last git tag.")
        sys.exit(1)

    # Ask user
    if (
        args.publish
        and input(
            f"We will create a release for tag {tag}. Do you want to continue? (y/n) "
        )
        != "y"
    ):
        print("exiting.")
        sys.exit(1)

    if args.publish:
        # Git push
        run_command(["git", "push", "--tags"])

        # Minisign password
        cmd = subprocess.run(["rbw", "get", "minisign"], capture_output=True, text=True)
        minisign_password = cmd.stdout

        # Create directory
        run_command(
            [
                "ssh",
                "akesi",
                # "-J", "pica01",
                "mkdir",
                "-p",
                f"/var/www/static/reaction/releases/{tag}/",
            ]
        )
    else:
        # Prepare directory for tarball and deb file.
        # We must do a `cargo clean` before each build,
        # So we have to move them out of `target/`
        local_dir = os.path.join(root_dir, "local")
        try:
            os.mkdir(local_dir)
        except FileExistsError:
            pass

    architectures = {
        "x86_64-unknown-linux-musl": "amd64",
        "aarch64-unknown-linux-musl": "arm64",
    }

    all_files = []

    instructions = [
        "## Changes",
        """
## Instructions

You'll need to install minisign to check the authenticity of the package.

After installing reaction, create your configuration file at
`/etc/reaction.json`, `/etc/reaction.jsonnet` or `/etc/reaction.yml`.
You can also provide a directory containing multiple configuration files in the previous formats.
See <https://reaction.ppom.me> for documentation.

Reload systemd:
```bash
$ sudo systemctl daemon-reload
```

Then enable and start reaction with this command
```bash
# replace `reaction.jsonnet` with the name of your configuration file in /etc/
$ sudo systemctl enable --now reaction@reaction.jsonnet.service
```
""".strip(),
    ]

    for (architecture_rs, architecture_pretty) in architectures.items():
        # Cargo clean
        run_command(["cargo", "clean"])

        # Install toolchain
        run_command(
            [
                "rustup",
                "toolchain",
                "install",
                f"stable-{architecture_rs}",
                "--force-non-host",  # I know, I know!
                "--profile",
                "minimal",
            ]
        )

        # Build
        run_command(["cross", "build", "--release", "--target", architecture_rs])

        # Build .deb
        cmd = run_command(
            ["cargo-deb", f"--target={architecture_rs}", "--no-build", "--no-strip"]
        )

        deb_dir = os.path.join("./target", architecture_rs, "debian")
        deb_name = [f for f in os.listdir(deb_dir) if f.endswith(".deb")][0]
        deb_path = os.path.join(deb_dir, deb_name)

        # Archive
        files_path = os.path.join("./target", architecture_rs, "release")
        pkg_name = f"reaction-{tag}-{architecture_pretty}"
        tar_name = f"{pkg_name}.tar.gz"
        tar_path = os.path.join(files_path, tar_name)

        os.chdir(files_path)
        try:
            os.mkdir(pkg_name)
        except FileExistsError:
            pass

        files = [
            # Binaries
            "reaction",
            "nft46",
            "ip46tables",
            # Shell completion
            "reaction.bash",
            "reaction.fish",
            "_reaction",
            # Man pages
            "reaction.1",
            "reaction-flush.1",
            "reaction-show.1",
            "reaction-start.1",
            "reaction-test-regex.1",
            "reaction-test-config.1",
        ]
        for file in files:
            shutil.copy(file, pkg_name)

        makefile = os.path.join(root_dir, "packaging", "Makefile")
        shutil.copy(makefile, pkg_name)

        systemd = os.path.join(root_dir, "config", "reaction.service")
        shutil.copy(systemd, pkg_name)

        run_command(["tar", "czf", tar_name, pkg_name])

        os.chdir(root_dir)

        if args.publish:
            # Sign
            run_command(
                ["minisign", "-Sm", deb_path, tar_path],
                text=True,
                input=minisign_password,
            )
            deb_sig = f"{deb_path}.minisig"
            tar_sig = f"{tar_path}.minisig"

            # Push
            run_command(
                [
                    "rsync",
                    "-az",  # "-e", "ssh -J pica01",
                    tar_path,
                    tar_sig,
                    deb_path,
                    deb_sig,
                    f"akesi:/var/www/static/reaction/releases/{tag}/",
                ]
            )

            all_files.extend([tar_path, tar_sig, deb_path, deb_sig])

            # Instructions

            instructions.append(
                f"""
## Tar installation ({architecture_pretty} linux)

```bash
curl -O https://static.ppom.me/reaction/releases/{tag}/{tar_name} \\
     -O https://static.ppom.me/reaction/releases/{tag}/{tar_name}.minisig \\
        && minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX -m {tar_name} \\
        && rm {tar_name}.minisig \\
        && cd {tar_name} \\
        && sudo make install
```
        """.strip()
            )

            instructions.append(
                f"""
## Debian installation ({architecture_pretty} linux)

```bash
curl -O https://static.ppom.me/reaction/releases/{tag}/{deb_name} \\
     -O https://static.ppom.me/reaction/releases/{tag}/{deb_name}.minisig \\
        && minisign -VP RWSpLTPfbvllNqRrXUgZzM7mFjLUA7PQioAItz80ag8uU4A2wtoT2DzX -m {deb_name} \\
        && rm {deb_name}.minisig \\
        && sudo apt install ./{deb_name}
```
        """.strip()
            )
        else:
            # Copy
            run_command(["cp", tar_path, deb_path, local_dir])

    if not args.publish:
        return

    # Release
    cmd = run_command(
        ["rbw", "get", "framagit.org", "token"], capture_output=True, text=True
    )
    token = cmd.stdout.strip()
    if token == "":
        print("Could not retrieve token")
        sys.exit(1)

    # Make user edit the description
    tmpdir = tempfile.TemporaryDirectory()
    desc_path = tmpdir.name + "/description.md"
    with open(desc_path, "w+") as desc_file:
        desc_file.write("\n\n".join(instructions))
    run_command(["vi", desc_path])

    with open(desc_path) as desc_file:
        description = desc_file.read().strip()

    if description == "":
        print()
        print("User deleted emptied description, exiting.")
        sys.exit(1)

    # Construct JSON payload
    files = [os.path.basename(file) for file in all_files]
    data = {
        "tag_name": tag,
        "description": description,
        "assets": {
            "links": [
                {
                    "url": "https://"
                    + f"static.ppom.me/reaction/releases/{tag}/{os.path.basename(file)}".replace(
                        "//", "/"
                    ),
                    "name": file,
                    "link_type": "other" if file.endswith(".minisig") else "package",
                }
                for file in files
            ]
        },
    }

    body = json.dumps(data)
    print(body)

    # Send POST request
    headers = {
        "Host": "framagit.org",
        "Content-Type": "application/json",
        "PRIVATE-TOKEN": token,
    }

    conn = http.client.HTTPSConnection("framagit.org")
    conn.request("POST", "/api/v4/projects/90566/releases", body=body, headers=headers)
    response = conn.getresponse()
    body = json.loads(response.read())

    if response.status != 201:
        print(
            f"sending message failed: status: {response.status}, reason: {response.reason}, message: {body.message}"
        )
        sys.exit(1)


main()
