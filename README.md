# reaction

A daemon that scans program outputs for repeated patterns, and takes action.

A common usage is to scan ssh and webserver logs, and to ban hosts that cause multiple authentication errors.

🚧 This program hasn't received external security audit yet. However, it already works well on many servers 🚧

> ℹ️🤖 The maintainer doesn't use LLMs.
>
> The AGENTS.md & CLAUDE.md files are just here to "discourage" LLMs to interact with this repository.

## Rationale

I was using the honorable fail2ban since quite a long time, but I was a bit frustrated by its CPU consumption
and all its heavy default configuration.

In my view, a security-oriented program should be simple to configure
and an always-running daemon should be implemented in a fast*er* language.

reaction does not have all the features of the honorable fail2ban, but it's more than 10x faster and has more manageable configuration.

[📽️ quick french name explanation 😉](https://u.ppom.me/reaction.webm)

[🇬🇧 in-depth blog article](https://blog.ppom.me/en-reaction)
/ [🇫🇷 french version](https://blog.ppom.me/fr-reaction)

## Rust rewrite

reaction v2.x is a complete Rust rewrite of reaction.
It's in feature parity with the Go version, v1.x, which is now deprecated.

See https://blog.ppom.me/en-reaction-v2.

## Configuration

YAML and [JSONnet](https://jsonnet.org/) (more powerful) are supported.
both are extensions of JSON, so JSON is transitively supported.

- See the [wiki](https://reaction.ppom.me/intro/) for configuration reference, multiple examples, security recommendations and FAQ.
- See [server.jsonnet](https://reaction.ppom.me/examples/configurations/ppom/server-jsonnet/) for a real-world configuration
- See [reaction.service](./config/reaction.service) for a systemd service file
- This minimal example (ipv4 only) shows what's needed to prevent brute force attacks on an ssh server (please read at least the [Security](https://reaction.ppom.me/good-practices/security/) part of the wiki before starting 🆙):

<details open>

<summary><code>/etc/reaction.yml</code></summary>

```yaml
patterns:
  ip:
    type: ipv4

start:
  - [ 'iptables', '-w', '-N', 'reaction' ]
  - [ 'iptables', '-w', '-I', 'INPUT',   '-p', 'all', '-j', 'reaction' ]
  - [ 'iptables', '-w', '-I', 'FORWARD', '-p', 'all', '-j', 'reaction' ]

stop:
  - [ 'iptables', '-w', '-D', 'INPUT',   '-p', 'all', '-j', 'reaction' ]
  - [ 'iptables', '-w', '-D', 'FORWARD', '-p', 'all', '-j', 'reaction' ]
  - [ 'iptables', '-w', '-F', 'reaction' ]
  - [ 'iptables', '-w', '-X', 'reaction' ]

streams:
  ssh:
    cmd: [ 'journalctl', '-fu', 'sshd.service' ]
    filters:
      failedlogin:
        regex:
          - 'authentication failure;.*rhost=<ip>'
          - 'Failed password for .* from <ip>'
          - 'Invalid user .* from <ip>'
          - 'banner exchange: Connection from <ip> port [0-9]*: invalid format'
        retry: 3
        retryperiod: '6h'
        actions:
          ban:
            cmd: [ 'iptables', '-w', '-I', 'reaction', '1', '-s', '<ip>', '-j', 'DROP' ]
          unban:
            cmd: [ 'iptables', '-w', '-D', 'reaction', '1', '-s', '<ip>', '-j', 'DROP' ]
            after: '48h'
```

</details>

<details>

<summary><code>/etc/reaction.jsonnet</code></summary>

```jsonnet
local banFor(time) = {
  ban: {
    cmd: ['iptables', '-w', '-A', 'reaction', '-s', '<ip>', '-j', 'DROP'],
  },
  unban: {
    cmd: ['iptables', '-w', '-D', 'reaction', '-s', '<ip>', '-j', 'DROP'],
    after: time,
  },
};

{
  patterns: {
    ip: {
      type: 'ipv4',
    },
  },
  start: [
    ['iptables', '-N', 'reaction'],
    ['iptables', '-I', 'INPUT', '-p', 'all', '-j', 'reaction'],
    ['iptables', '-I', 'FORWARD', '-p', 'all', '-j', 'reaction'],
  ],
  stop: [
    ['iptables', '-D', 'INPUT', '-p', 'all', '-j', 'reaction'],
    ['iptables', '-D', 'FORWARD', '-p', 'all', '-j', 'reaction'],
    ['iptables', '-F', 'reaction'],
    ['iptables', '-X', 'reaction'],
  ],
  streams: {
    ssh: {
      cmd: ['journalctl', '-fu', 'sshd.service'],
      filters: {
        failedlogin: {
          regex: [
            @'authentication failure;.*rhost=<ip>',
            @'Failed password for .* from <ip>',
            @'banner exchange: Connection from <ip> port [0-9]*: invalid format',
            @'Invalid user .* from <ip>',
          ],
          retry: 3,
          retryperiod: '6h',
          actions: banFor('48h'),
        },
      },
    },
  },
}
```

</details>

> It is recommended to setup reaction with [`nftables`](https://reaction.ppom.me/examples/actions/nftables/)
> or [`ipset` + `iptables`](https://reaction.ppom.me/examples/actions/ipset/), which are much more performant
> solutions than `iptables` alone.

### Database

The embedded database is stored in the working directory (but can be overriden by the `state_directory` config option).
If you don't know where to start reaction, `/var/lib/reaction` should be a sane choice.

### CLI

- `reaction start` runs the server
- `reaction show` show pending actions (ie. show current bans)
- `reaction flush` permits to run pending actions (ie. clear bans)
- `reaction trigger` permits to manually trigger a filter (ie. run custom ban)
- `reaction test-regex` permits to test regexes
- `reaction test-config` shows loaded configuration
- `reaction help` for full usage.

### old binaries

`ip46tables` and `nft46` binaries are no longer part of reaction. If you really need them, see
[the last commit that included them](https://framagit.org/ppom/reaction/-/tree/b7d997ca5e9a69c8572bb2ec9d27d0eb03b3cb9f/helpers_c).

## Wiki

You'll find more ressources, service configurations, etc. on [the wiki](https://reaction.ppom.me)!

We recommend that you read the ***Good Practices*** chapters before starting.

## Installation

[![Packaging status](https://repology.org/badge/vertical-allrepos/reaction-fail2ban.svg)](https://repology.org/project/reaction-fail2ban/versions)

### Binaries

Executables and .deb packages are provided [in the releases page](https://framagit.org/ppom/reaction/-/releases/), for x86-64/amd64 linux and aarch64/arm64 linux.

Signature verification and installation instructions are provided in the releases page.

> Provided binaries are compiled by running `nix-shell release.py` on a NixOS machine with docker installed.

#### NixOS

reaction is packaged, but the [**module**](https://framagit.org/ppom/nixos/-/blob/main/modules/common/reaction.nix) has not yet been upstreamed.

#### OpenBSD

See the [wiki](https://reaction.ppom.me/examples/configurations/openbsd/).

### Compilation

You'll need a recent rust toolchain. `rustup` is recommended way to install it.
```shell
$ make
```
Don't hesitate to take a look at the `Makefile` to understand what's happening!

### Installation

To install the binaries
```shell
make install
```

To install the systemd file as well
```shell
make install_systemd
```

## Contributing

> We, as participants in the open source ecosystem, are ethically responsible for the software
> and hardware we help create - as it can be used to perpetuate inequalities or help empower
> marginalized communities, and fight against patriarchy, capitalism, sexism, gender violence,
> racism, ableism, homophobia, colonialism, fascism, surveillance, and oppressive control.

- [NGI's Diversity and Inclusion Guide](https://nlnet.nl/NGI0/bestpractices/DiversityAndInclusionGuide-v4.pdf)

I'll do my best to maintain a safe contribution place, as free as possible from discrimination and elitism.

### Ideas

Please take a look at issues which have the "Opinion Welcome 👀" label!
*Your opinion is welcome.*

Your ideas are welcome in the issues.

### Code

Contributions are welcome.

For any substantial feature, please file an issue first, to be assured that we agree on the feature, and to avoid unnecessary work.

I recommend reading [`ARCHITECTURE.md`](ARCHITECTURE.md) first. This is a quick tour of the codebase, which should save time to new contributors.

You can also join this Matrix development room: [#reaction-dev-en:club1.fr](https://matrix.to/#/#reaction-dev-en:club1.fr).
French version: [#reaction-dev-fr:club1.fr](https://matrix.to/#/#reaction-dev-fr:club1.fr).

## Help

You can ask for help in the issues or in this Matrix room: [#reaction-users-en:club1.fr](https://matrix.to/#/#reaction-users-en:club1.fr).
French version: [#reaction-users-fr:club1.fr](https://matrix.to/#/#reaction-users-fr:club1.fr).
You can alternatively send a mail: `reaction` on domain `ppom.me`.

## Funding

<!-- This is a free time project, so I'm not working on schedule.
However, if you're willing to fund the project, I can priorise and plan paid work. This includes features, documentation and specific JSONnet configurations. -->

This project is currenlty funded through the [NGI0 Core](https://nlnet.nl/core) Fund, a fund established by [NLnet](https://nlnet.nl) with financial support from the European Commission's [Next Generation Internet](https://ngi.eu) programme.

![NLnet logo](logo/nlnet.svg)
