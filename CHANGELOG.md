## Changelog

## v2.8.4

- Add `reaction-plugin-syslog`, an input plugin for UDP and TCP syslog streams with RFC 3164/RFC 5424 parsing, metadata selectors and safe prefix templates.

## v2.7.1

Small improvements and fixes.

### Lock file

I hope this is the last iteration on lock files!
Lock files are now unix "advisory locks". This means that the kernel itself releases the lock on process exit.
False positives should now be impossible: reaction should never refuse to start if there isn't already a reaction instance running.

Thanks to @bket and @msfjarvis for their merge requests !39 !40

### reaction version

reaction now logs its version on startup, before loading the configuration.
Useful if you have an error because you forgot to update before using a new configuration option.

### Build fix

For v2.7.0, I had to fork the ipset library because my bugfix haven't been upstreamed yet.
I merged another bugfix, to fix a compilation error on some Rust versions.

Thanks to @patrickl for the issue and fix #159


## v2.7.0

This release is full of breaking changes! For the greater good, I prommy.

> It's a lot of small changes, life improvements that have been asked since a long time and that I didn't take time to address.
> 6 issues were addressed, some of them were 2 years old.
> During the past year I mainly focused on bigger features that were part of the NLnet roadmap.
>
> This release is the last release that is financially backed by NLnet. Thanks again to them. It's been super helpful to this project and to my personal stability.
>
> Fear not! I'll continue working on reaction and maintaining it, at a slower pace.
> I consider reaction's core to be mostly finished.
> But there is plenty of room for new plugins.
>
> I'm very excited to see what reaction's feature set will look like in 2 years!

### `reaction test-filter`

First, a non-breaking change! A new CLI command.

Similar to `reaction test-regex`, `reaction test-filter` permits to test a Filter in your configuration against some lines that you give to its stdin.
This permits to avoid copy pasting regexes from your configuration file in your CLI.

### Breaking change: Action's `env` and `pass_env`

reaction now resets the environment of its Actions.

You can set the environment with the `env` option:

```jsonnet
{
  mail: {
    cmd: ["curl", "--variable", "%PASSWORD", "--password", "{{PASSWORD}}"],
    env: {
      PASSWORD: importstr "/var/secrets/password.txt",
    },
  },
}
```

If you want to pass an environment variable from reaction's environment, the `pass_env` option permits to pass them as-is:

```jsonnet
{
  mail: {
    cmd: ["curl", "--variable", "%PASSWORD", "--password", "{{PASSWORD}}"],
    pass_env: [
      "PASSWORD"
    ],
  },
}
```

### Breaking change: Directories

`state_directory` doesn't default to reaction's current directory anymore (`.`), but to `/var/lib/reaction`.
This shouldn't affect most users.

`runtime_directory` is a new top-level option. It defaults to `/run/reaction`.

- The socket name's defaults to be named `reaction.sock` inside that directory. So its default path doesn't change: `/run/reaction/reaction.sock`.
  - The daemon's CLI option `-s`/`--socket` still works and overrides that.
- The database lock file defaults to be in this directory and is named `reaction.lock`. So it's not in the state_directory anymore, but defaults to `/run/reaction/reaction.lock`.
  - This folder is meant to be cleaned up by the service file, and solves the issue of the lock file not being removed if reaction is SIGKILLed or the VM unexpectedly crashes/loses power... Preventing reaction from starting at next start, because there is still a lock file.

### Log level

`log_level` is another new-level option. It's the same as the CLI `-l`/`--loglevel` option. The CLI option still takes precedence.

> This option was added to facilitate the edition of this option, without editing the init file.

### `-c`/`--config` option is optional

The CLI `-c`/`--config` option is now optional on all CLI subcommands and defaults to `/etc/reaction`.

### `DEBUG` log level

When setting the log level to DEBUG, reaction now prints the stdout and stderr of the actions that exit with a non-zero exit code.
This helps when trying to understand why an action doesn't work as expected!

Additionally, reaction now properly prints exit code when non-zero.

## v2.6.0

This release brings a bugfix for the "ipset" plugin, a new top-level option for plugins, and the much awaited "HTTP ban" plugin.

### ipset plugin bugfix

When using a Pattern with type `ip` or `ipv6`, that did not specify an `ipv6mask`, ipset converted the IP to its /32 network mask.
This led to much wider bans than wanted!

This was due to a bug in the `ipset` rust crate when parsing IPv6 addresses.
A PR is on its way upstream, and we use a patched version of the library until then.

Thanks to @celforyon for the alert on this bug.

### Top-level `options` for plugins.

Plugins can now take top-level options, for configuration that can't live in the Streams or Actions sections.

```yaml
plugins:
  myplugin:
    path: ...
    options:
      owo: true
      list:
        - name: one
        - name: two
      # ...
```

If your configuration lives accross multiple files, the values will be merged recursively.
You can use `reaction test-config` to test how your conf is evaluated.

[Plugin options documentation reference](https://reaction.ppom.me/reference/plugin/#options)

Existing `ipset` and `nftables` plugins will use this in the future. Their configuration stays unchanged for now.
But this feature was much needed for the following new plugin...

### HTTP ban plugin 🎉

It's been 2 years since I first thought of this plugin.
Since then, [Anubis](https://anubis.techaro.lol) and [iocaine](https://iocaine.madhouse-project.org/) were born.
They have different, complementary approaches, and are more powerful than reaction when protecting from (or fighting) LLM scrapers.

In my opinion, the current main goal of reaction is to protect from low-stakes cyberattacks by hiding the server from reconnaissance bots scanning the web, the IPv4 space, etc.

But currently, when false positives happen, the IP is banned at the firewall level.
Legitimate users just see that the "website disappeared and can't load".
This is inconvenient.
The HTTP ban plugin addresses the issue.

The plugin is an HTTP middleware: it sits between your internet-facing reverse-proxy, and your web application servers.
It will triage the incoming requests, presenting an error page to banned IPs, and routing the legitimate traffic to web apps.

[HTTP ban documentation](https://reaction.ppom.me/plugins/httpban/)

## v2.5.1

This is a bugfix release.
When killed, reaction would restart its streams with behavior: 'restart' (the new default), never exiting gracefully.

## v2.5.0

This release brings one new functionality, with a breaking change.

1. Handling failing streams

Until this release, when the command of a stream failed, reaction would log an error and ignore this failed stream.
It kept running as long as there was at least one active stream left.
It is now possible to configure this behavior. When the command of a stream fails, reaction can either ignore it (old behavior), exit, or restart the stream (new default behavior).
It is possible to configure how long to wait before a restarting, how many retries to give before giving up, and what to do when we give up (ignore or exit).
reaction now defaults to restarting the stream command when it exits, indefinitely, with a 5s pause.
[See all those options in the documentation reference.](https://reaction.ppom.me/reference/stream/#eof)

2. Potential breaking changes

The libraries for YAML and JSONnet have been updated.
I have tested my configuration files, which use both YAML and JSONnet, and saw no difference.
While unlikely, some edge-cases can possibly be handled differently in those new versions.
Please report any configuration that breaks will 2.5.0, and worked in previous 2.x releases!

## v2.4.2

This release fixes two bugs introduced in 2.4.0.

1. Persisted actions in a type-struct filter were not run at start
The problem concerned filters with type: struct, which parse JSON.
Their actions were not run at start.
They were correctly persisted in-DB though, so a reaction restart from 2.4.0 (or 2.4.1) to 2.4.2 will run those forgotten actions.

2. `test-regex` didn't show ignores.
A minor bug: reaction test-regex didn't show ignored patterns anymore. This has been fixed.

## v2.4.1

1. Bug fix
This release fixes a bug on Filters that use the new JSON feature:
Ignores (`ignore`, `ignoreregex`, `ignorecidr`) on patterns were not taken into account when using type: struct Filters.

2. Compilation fix

This release fixes a compilation issue on aarch64 for the nftables plugin.

## v2.4.0

1. New feature

reaction can now parse JSON logs!
Instead of a regex, you can match JSON logs in a structured way:
```yaml
patterns:
  ip:
    type: ip

streams:
  cmd: ['tail', '-Fn0', '/var/log/traefik.log']
  filters:
    # Before ☹️
    regexfilter:
      regex:
        - '^.*"ClientHost":"<ip>".*"request_User-Agent":"[^"]*(ClaudeBot|GPTBot)[^"]*"'
    # After 😎
    structfilter:
      type: 'struct'
      options:
        lang: 'json'
        fields:
          ClientHost: '<ip>'
          'request_User-Agent': '.*(ClaudeBot|GPTBot).*',
```

Careful attention has been paid to optimization.
Therefore, structured matching performance is at least on par with regex matching.
It showed 1.2x better performance in tests against real-life traefik logs with 2 different filters.
See all related options in the configuration reference.

2. Database updates

The database saw some notable changes this month:

2.1. Rotation

reaction's database is very simple: each match and each action ran is persisted as a JSON line in the reaction.db file.
Relevant data is also stored in RAM.
This append-only log of all events permits to reduce I/O operations and to have RAM speed operations.
Before this release, the database was "rotated" (i.e. garbage collected) at start, and then each time 20MiB were written to disk.
This was fine for most users, but made the performance collapse under heavy load.
Instead of rotating based on the size of the DB, reaction now rotates in a timely manner.
It always rotates the DB on start, then it defaults to rotating the DB every 24h.
See the configuration reference if you want to change this interval.

2.2. Lockfile

There is now a `reaction.lock` lockfile against the `reaction.db` file, protecting against accidental start of two reaction processes.
reaction will now refuse to start if this lockfile is present. If reaction hasn't cleanly exited (e.g. because of power outage), you can safely delete the lockfile.

3. Bugfixes

3.1. Database error

In some cases, the database getting a fatal error (e.g. "No space left on device") didn't stop reaction from running, stucking it in a weird state.
This has been fixed: every fatal database error now causes reaction to exit.

3.2. Actions ran twice on startup

On startup, reaction started each action once for each regex in the underlying filter.
One regex? Each action is triggered once. Two regexes? Each action is triggered twice. Etc.
It's notoriously hard to test against each possibility, and this WTF error is a great example 😅

## 2.3.1

1. reaction

Add useful error messages for plugins.

2. packaging

Move reaction and its plugins to a common systemd "slice".

This permits to apply resource limits to both reaction and its plugins, by assigning those limits to their slice.

3. reaction-plugin-ipset

v1.0.1 fixes some inconsistencies when start fails.
It also prints logs in a more coherent way, permitting easier log reading.

This plugin is still beta. If you have issues with it, don't hesitate to:
- reach out,
- rollback to using [classic ipset actions](https://reaction.ppom.me/examples/actions/ipset/).

4. reaction-plugin-nftables

First beta version of the nftables plugin!

An nftables plugin, to perform nftables actions using less resources than with classic actions, and making it easier to have one ip set per reaction filter.
 *Its configuration is very close to the ipset plugin, with only idiomatic differences between ipset/nftables.*

Documentation is available on the wiki: <https://reaction.ppom.me/plugins/nftables/>.

This plugin is beta. If you have issues with it, don't hesitate to:
- reach out,
- rollback to using [classic nftables actions](https://reaction.ppom.me/examples/actions/nftables/).

## 2.3.0

1. Plugins!

reaction includes now [a plugin system](https://reaction.ppom.me/plugins/).

This permits big gains in performance, better usability and extensibility.

Two plugins have been released alongside the v2.3.0:

1.1. virtual

Enables the use of "virtual" Streams and "virtual" Actions.
This permits second-level streams, for example to ban recidivist IPs for a longer time.

1.2. ipset

Permits to manipulate ipsets *much faster* than with UNIX commands.

> You'll have to install the `libipset-dev` package on Debian to be able to run this plugin.

**See the [Plugin documentation](https://reaction.ppom.me/plugins/) to learn more.**

See also the [Developer documentation](https://docs.rs/reaction-plugin) to learn how to create your own plugin (the documentation is in docs.rs's build queue at the time of publishing).

1.3. Upcoming plugins

A sibling of the ipset plugin, for nftables, will probably be the next new plugin.

Then more ambitious plugins are on the roadmap,
see the [previous blog post](https://blog.ppom.me/en-reaction-v2/#json)
for more information.

2. Performance

Usage of an external library to manage time operations was considerably reduced.
This reduces both CPU and RAM usage, up to 10% less CPU and 20% less RAM used.

3. Breaking changes

3.1. systemd service

**Note to maintainers: systemd service changed, please update accordingly!**

1. `KillMode=mixed` has been added. It's important for the plugin system to quit gracefully.

2. `ExecStart` now specify `/etc/reaction/` directory as a target.
This permits to have a unified command, whereas you use json, yaml, jsonnet, or a directory.

If you used the debian package:
After creating `/etc/reaction/` (`mkdir -p /etc/reaction`), move your configuration file into `/etc/reaction/`.
Stop and disable your templated unit with `systemctl disable --now reaction@xxx.service` (replace `xxx`)
and restart with `systemctl enable --now reaction.service`.

3.2. ip46tables and nft46 removal

ip46tables and nft46 are not included in reaction's package anymore.

There is now a more performant solution built in reaction, see [v2.2.0 release](https://framagit.org/ppom/reaction/-/releases/v2.2.0#special-ip-support) or the [reference documentation](https://reaction.ppom.me/reference/action/#type).

For iptables users:
- It is advised to use the [ipset framework](https://reaction.ppom.me/examples/actions/ipset/) instead of plain iptables, for much better performance.
- It is even advised to use the brand new official ipset plugin, which features even better performance.

For nftables users:
- It is advised to follow the [wiki page on nftables](https://reaction.ppom.me/examples/actions/nftables/) which shows up to date documentation.

If for some reason you really need to stay on `ip46tables` or `nft46`, you can take the binaries from the [previous release](https://framagit.org/ppom/reaction/-/releases/v2.2.1) and move them in `/usr/local/bin`, or build them from reaction repository with `make nft46` and `make ip46tables`.

4. Notes on precompiled binaries

I used to cross compile packages for 3 different architectures with the cross-rs project.
However it broke for reaction when making this release, and after too much hours trying to make it work again, I gave up.

So this release sadly **only features prebuilt binaries for x86_64-unknown-linux-gnu**.
It works on Debian 12, Debian 13 and their derivatives.

If **anyone can help to bring back cross-compilation**, I'd be really glad!
See the [Code and Help](https://framagit.org/ppom/reaction/#code) sections of the README.

## v2.2.1

This release fixes some bugs related to persistance.
Especially, some actions (IPs...) in database were ignored by reaction on next startup and made reaction unable to ban this IP again.
This persistence issue was related to database writes, whose order was not guaranteed.
This has been fixed, and removed in the same time a spike in RAM usage during startup.

## v2.2.0

This release brings two substantial features to reaction 🎉

1. Duplicates handling (breaking change!)

reaction's filters used to execute the same actions multiple times.

For example, when an IP address was spamming the server, due to the latency
between the moment the request was received and the moment the ban action was
executed by reaction, and taken into account by the firewall, other logs
concerning this IP address could appear, resulting in a new trigger of the
reaction filter.
Therefore an IP could be banned multiple times, resulting in bugs on some
firewalls that deduplicate IPs, like `ipset` and `nftables`.

The new behavior defaults to extending the trigger period.

Let's take this filter as an example:
```jsonnet
{
  regex: [
    @'Failed password for .* from <ip>',
  ],
  retry: 3,
  retryperiod: '6h',
  actions: {
    ban: {
      cmd: ['iptables', '-w', '-A', 'reaction', '-s', '<ip>', '-j', 'DROP'],
    },
    unban: {
      cmd: ['iptables', '-w', '-D', 'reaction', '-s', '<ip>', '-j', 'DROP'],
      after: '2h',
    },
  },
}
```

If a filter is triggered at 0:00, the `ban` action runs, and the `unban` action is scheduled at 2:00.
If for some reason, some new matches appear at 0:10 (for the example's sake):
- Before this release, reaction would launch a new `ban` command, and schedule an `unban` command at 2:10.
- Now, reaction defaults to extending the `unban`. So it won't schedule a new `ban` command. It will instead reschedule the existing `unban` action to 2:10.
- This new default behavior can be explicitly specified by:
  ```jsonnet
  duplicate: 'extend' // the default
  ```
- Old behavior is possible by adding this parameter:
  ```jsonnet
  duplicate: 'rerun'
  ```
- Another behavior is possible by adding this parameter:
  ```jsonnet
  duplicate: 'ignore'
  ```
  It will simply ignore new matches for this IP.

2. Special IP support

2.1. Built-in regex

Before this release, admins had to specify themselves the regex for an IP.
It's now built-in:

```jsonnet
{
  patterns: {
    ip: {
      // IPv4 and IPv6
      type: 'ip',
    },
    ip4: {
      // IPv4 only
      type: 'ipv4',
    },
    ip6: {
      // IPv6 only
      type: 'ipv6',
    },
  },
}
```

In addition to being built-in, the new regexes have been heavily tested and consolated.

Before, a regex like this could catch only a fraction of an IP, because `.*` is greedy:
```jsonnet
regex: [
  "Failed password: .*<ip> .*"
]
```
The `.*` could "eat" parts of the IP.
The new regexes are now safe from these.

> I'll still advise to specify the delimiter characters
> before and after the <ip>, to speed-up the regex engine:
> ```jsonnet
> regex: [
>   "Failed password: .*, <ip> .*"
> ]
> ```

2.2. CIDR notation for ignore

You can now ignore full CIDR.
This is more robust that `ignoreregex`, as some IP masks can crossover digits.

```jsonnet
{
  patterns: {
    ip: {
      type: 'ip',
      ignorecidr: [
        '192.168.1.0/24',
        'abcd:1234:2345:3456::/64',
      ],
    },
  },
}
```

2.3. Grouping IP matches

As IPv6 are veeery cheap: malicious actors typically have 2^64 IPv6, with a /64 network mask. This is common even on residential IPs.

reaction now permits to group matches by network mask:

```jsonnet
{
  patterns: {
    ip: {
      type: 'ip',
      ipv6mask: 64,
    },
  },
}
```

So those IP will be grouped:

```
2001:db8:2345:3456::1
2001:db8:2345:3456::2
2001:db8:2345:3456::3
```

And the action will be run with the network mask:
```
2001:db8:2345:3456::/64
```

This is also possible for IPv4. Be careful doing this!
Some actors may have only 1, 2, 4 IPs from a range, so this may be a bad idea.

```jsonnet
{
  patterns: {
    ip: {
      type: 'ip',
      ipv4mask: 30, // 24 ...
      ipv6mask: 64,
    },
  },
}
```

2.4. Actions on one IP version

You can now run different actions depending on the IP version:
```jsonnet
actions: {
  // Will run only for IPv4
  ban4: {
    cmd: ['iptables', '...', '<ip>'],
    ipv4only: true,
  },
  // Will run only for IPv6
  ban6: {
    cmd: ['ip6tables', '...', '<ip>'],
    ipv6only: true,
  },
  // ...
}
```

2.5 `ip46tables` and `nft46` deprecation

With those new features, the `ip46tables` and `nft46` binaries that I wrote in C for dynamically dispatching `iptables` and `ip6tables` depending on the IP type are no longer useful.
I will continue to build and ship them on the `2.2.x` releases, but will stop soon after.

While I encourage you to use the new features, if you don't like change, you can also copy those binaries in your `/usr/local/bin`. They are statically compiled and ~70 SLOC, so they should be time-proof.

---

The [example configurations](https://framagit.org/ppom/reaction/-/tree/main/config) have been updated, if you want to take a look at a real example of IP pattern use :eyes:

## 2.1.2

This is a bugfix release.

Since 2.0.0, triggers were not persisted to database long enough: they were persisted for the period of the retry period, and not for the last `after` action duration.

So for example with this config:
```yaml
streams:
  ssh:
    cmd: [ 'journalctl', '-n0', '-fu', 'sshd.service' ]
    filters:
      failedlogin:
        regex: ....
        retry: 3
        retryperiod: 6h
        actions:
          ban:
            cmd: ...
          unban:
            cmd: ...
            after: '2 days'
```

reaction would store actions to run, only for 6 hours, and not for 2 days.

Restarting reaction made it "forget" those actions to run, earlier than expected.

You'll have to restart reaction to upgrade, so it will "forget" its pending actions too early for the last time.
Next restarts won't forget anything.

## 2.1.1

Trailing newlines (`\n`) were not trimmed from the log lines anymore,
resulting in regexes finishing with $ (end of line) not matching.
This release fixes the bug.

## 2.1.0

1. New Features

1.1. One shot actions #92

Actions with `oneshot: true` will be run only the first time.
They won't be run again when reaction restarts.
It's useful for alerting for example, if you want to receive an alert when an IP is banned, but not when reaction restarts.

See an example [here (mail)](https://framagit.org/ppom/reaction/-/blob/78056b6fc5e6030d02353fd36799f9231126f4a0/config/example.yml#L107-111) and another [here (abuseipdb reporting)](https://reaction.ppom.me/actions/abuseipdb.html).

1.2. Trigger command #93

reaction now features a `reaction trigger` command, that permit to manually trigger a filter, i.e. manually creating bans on a filter.

Example:
```bash
reaction trigger ssh.failedlogin ip=1.2.3.4
```

2. Fixes

- Discard invalid UTF8 sequences in streams, instead of closing the stream. #124
- Improve error message when `retry` is specified but is `<= 1`. #125
- Configuration evaluation order is now predictable. That is, files are read in order by name. This won't change anything, unless for start/stop commands order. #126

3. Performance

Up to x12 performance improvement (and beyond!) on regex matching.
The more regexes in a stream, greater the performance gain is.

Tests performed on my homeserver's webserver last 6 months of logs:

Before:
```
CPU time consumed: 3min 44.299s
```
After:
```
CPU time consumed: 21.998s
```

Time executing Actions is not taken into account, so if your actions are heavy on CPU time, you'll only see a smaller improvement.

4. Build

Fix build on environments without `gcc`, falling back to `$CC` or `cc`.

## 2.0.1

Fix packaging issues:

- C binaries were not cross-compiled on arm64
- Tarballs' Makefile errored when doing 'make install'

You only benefit to upgrade from 2.0.0 if you ran into issues while installing.

## 2.0.0

See the [blog article](https://blog.ppom.me/en-reaction-v2)!
