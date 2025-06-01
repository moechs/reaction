# reaction

A daemon that scans program outputs for repeated patterns, and takes action.

A common usage is to scan ssh and webserver logs, and to ban hosts that cause multiple authentication errors.

🚧 This program hasn't received external audit. however, it already works well on my servers 🚧

## Current project status

reaction just reached v2.0.0-rc2 version, which is a complete rust rewrite of reaction.
It's in feature parity with the Go version, and breaking changes should be small.

See https://reaction.ppom.me/migrate-to-v2.html

## Rationale

I was using the honorable fail2ban since quite a long time, but i was a bit frustrated by its cpu consumption
and all its heavy default configuration.

In my view, a security-oriented program should be simple to configure
and an always-running daemon should be implemented in a fast*er* language.

reaction does not have all the features of the honorable fail2ban, but it's ~10x faster and has more manageable configuration.

[📽️ quick french name explanation 😉](https://u.ppom.me/reaction.webm)

[🇬🇧 in-depth blog article](https://blog.ppom.me/en-reaction)
/ [🇫🇷 french version](https://blog.ppom.me/fr-reaction)

## Configuration

YAML and [JSONnet](https://jsonnet.org/) (more powerful) are supported.
both are extensions of JSON, so JSON is transitively supported.

- See [reaction.yml](./app/example.yml) or [reaction.jsonnet](./config/example.jsonnet) for a fully explained reference
- See [server.jsonnet](./config/server.jsonnet) for a real-world configuration
- See [reaction.example.service](./config/reaction.example.service) for a systemd service file
- This minimal example shows what's needed to prevent brute force attacks on an ssh server (please read at least the [Security](https://reaction.ppom.me/security.html) part of the wiki before starting 🆙):

<details open>

<summary><code>/etc/reaction.yml</code></summary>

```yaml
patterns:
  ip:
    regex: '(([0-9]{1,3}\.){3}[0-9]{1,3})|([0-9a-fA-F:]{2,90})'
    ignore:
      - '127.0.0.1'
      - '::1'

start:
  - [ 'ip46tables', '-w', '-N', 'reaction' ]
  - [ 'ip46tables', '-w', '-I', 'INPUT',   '-p', 'all', '-j', 'reaction' ]
  - [ 'ip46tables', '-w', '-I', 'FORWARD', '-p', 'all', '-j', 'reaction' ]

stop:
  - [ 'ip46tables', '-w', '-D', 'INPUT',   '-p', 'all', '-j', 'reaction' ]
  - [ 'ip46tables', '-w', '-D', 'FORWARD', '-p', 'all', '-j', 'reaction' ]
  - [ 'ip46tables', '-w', '-F', 'reaction' ]
  - [ 'ip46tables', '-w', '-X', 'reaction' ]

streams:
  ssh:
    cmd: [ 'journalctl', '-fu', 'sshd.service' ]
    filters:
      failedlogin:
        regex:
          - 'authentication failure;.*rhost=<ip>'
          - 'Failed password for .* from <ip>'
          - 'Invalid user .* from <ip>',
          - 'banner exchange: Connection from <ip> port [0-9]*: invalid format',
        retry: 3
        retryperiod: '6h'
        actions:
          ban:
            cmd: [ 'ip46tables', '-w', '-I', 'reaction', '1', '-s', '<ip>', '-j', 'DROP' ]
          unban:
            cmd: [ 'ip46tables', '-w', '-D', 'reaction', '1', '-s', '<ip>', '-j', 'DROP' ]
            after: '48h'
```

</details>

<details>

<summary><code>/etc/reaction.jsonnet</code></summary>

```jsonnet
local iptables(args) = [ 'ip46tables', '-w' ] + args;
local banFor(time) = {
  ban: {
    cmd: iptables(['-A', 'reaction', '-s', '<ip>', '-j', 'DROP']),
  },
  unban: {
    after: time,
    cmd: iptables(['-D', 'reaction', '-s', '<ip>', '-j', 'DROP']),
  },
};

{
  patterns: {
    ip: {
      regex: @'(?:(?:[ 0-9 ]{1,3}\.){3}[0-9]{1,3})|(?:[0-9a-fA-F:]{2,90})',
    },
  },
  start: [
    iptables([ '-N', 'reaction' ]),
    iptables([ '-I', 'INPUT',   '-p', 'all', '-j', 'reaction' ]),
    iptables([ '-I', 'FORWARD', '-p', 'all', '-j', 'reaction' ]),
  ],
  stop: [
    iptables([ '-D', 'INPUT',   '-p', 'all', '-j', 'reaction' ]),
    iptables([ '-D', 'FORWARD', '-p', 'all', '-j', 'reaction' ]),
    iptables([ '-F', 'reaction' ]),
    iptables([ '-X', 'reaction' ]),
  ],
  streams: {
    ssh: {
      cmd: [ 'journalctl', '-fu', 'sshd.service' ],
      filters: {
        failedlogin: {
          regex: [
            @'authentication failure;.*rhost=<ip>'
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


### Database

The embedded database is stored in the working directory (but can be overriden by the `state_directory` config option).
If you don't know where to start reaction, `/var/lib/reaction` should be a sane choice.

### CLI

- `reaction start` runs the server
- `reaction show` show pending actions (ie. bans)
- `reaction flush` permits to run pending actions (ie. clear bans)
- `reaction test-regex` permits to test regexes
- `reaction help` for full usage.

### `ip46tables`

`ip46tables` is a minimal c program present in its own subdirectory with only standard posix dependencies.

It permits to configure `iptables` and `ip6tables` at the same time.
It will execute `iptables` when detecting ipv4, `ip6tables` when detecting ipv6 and both if no ip address is present on the command line.

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

See the [wiki](https://reaction.ppom.me/configurations/OpenBSD.html).

### Compilation

You'll need a recent rust toolchain for reaction and a c compiler for ip46tables.
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
Your ideas are welcome in the issues.
Contributions are welcome.

For any substantial feature, please file an issue first, to be assured that we agree on the feature, and to avoid unnecessary work.

I recommend reading [`ARCHITECTURE.md`](ARCHITECTURE.md) first. This is a quick tour of the codebase, which should save time to new contributors.

You can also join this Matrix development room: [#reaction-dev-en:club1.fr](https://matrix.to/#/#reaction-dev-en:club1.fr).
French version: [#reaction-dev-fr:club1.fr](https://matrix.to/#/#reaction-dev-fr:club1.fr).

## Help

You can ask for help in the issues or in this Matrix room: [#reaction-users-en:club1.fr](https://matrix.to/#/#reaction-users-en:club1.fr).
French version: [#reaction-users-fr:club1.fr](https://matrix.to/#/#reaction-users-fr:club1.fr).

## Funding

<!-- This is a free time project, so I'm not working on schedule.
However, if you're willing to fund the project, I can priorise and plan paid work. This includes features, documentation and specific JSONnet configurations. -->

This project is currenlty funded through the [NGI0 Core](https://nlnet.nl/core) Fund, a fund established by [NLnet](https://nlnet.nl) with financial support from the European Commission's [Next Generation Internet](https://ngi.eu) programme.

![NLnet logo](logo/nlnet.svg)
