// This file is using JSONnet, a complete configuration language based on JSON
// See https://jsonnet.org
// JSONnet is a superset of JSON, so one can write plain JSON files if wanted.
// Note that YAML is also supported, see ./example.yml

// This example configuration file is a good starting point, but you're
// strongly encouraged to take a look at the full documentation: https://reaction.ppom.me

// JSONnet functions
local iptables(args) = ['ip46tables', '-w'] + args;
// ip46tables is a minimal C program (only POSIX dependencies) present in a
// subdirectory of this repo.
// it permits to handle both ipv4/iptables and ipv6/ip6tables commands

// See meaning and usage of this function around L106
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
  // patterns are substitued in regexes.
  // when a filter performs an action, it replaces the found pattern
  patterns: {
    ip: {
      // reaction regex syntax is defined here: https://docs.rs/regex/latest/regex/#syntax
      // jsonnet's @'string' is for verbatim strings
      // simple version: regex: @'(?:(?:[0-9]{1,3}\.){3}[0-9]{1,3})|(?:[0-9a-fA-F:]{2,90})',
      regex: @'(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)(?:\.(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)){3}|(?:(?:[0-9a-fA-F]{1,4}:){7,7}[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,7}:|(?:[0-9a-fA-F]{1,4}:){1,6}:[0-9a-fA-F]{1,4}|(?:[0-9a-fA-F]{1,4}:){1,5}(?::[0-9a-fA-F]{1,4}){1,2}|(?:[0-9a-fA-F]{1,4}:){1,4}(?::[0-9a-fA-F]{1,4}){1,3}|(?:[0-9a-fA-F]{1,4}:){1,3}(?::[0-9a-fA-F]{1,4}){1,4}|(?:[0-9a-fA-F]{1,4}:){1,2}(?::[0-9a-fA-F]{1,4}){1,5}|[0-9a-fA-F]{1,4}:(?:(?::[0-9a-fA-F]{1,4}){1,6})|:(?:(?::[0-9a-fA-F]{1,4}){1,7}|:)|fe80:(?::[0-9a-fA-F]{0,4}){0,4}%[0-9a-zA-Z]{1,}|::(?:ffff(?::0{1,4}){0,1}:){0,1}(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])|(?:[0-9a-fA-F]{1,4}:){1,4}:(?:(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9])\.){3,3}(?:25[0-5]|(?:2[0-4]|1{0,1}[0-9]){0,1}[0-9]))',
      ignore: ['127.0.0.1', '::1'],
      // Patterns can be ignored based on regexes, it will try to match the whole string detected by the pattern
      // ignoreregex: [@'10\.0\.[0-9]{1,3}\.[0-9]{1,3}'],
    },
  },

  // where the state (database) must be read
  // defaults to . which means reaction's working directory.
  // The systemd service starts reaction in /var/lib/reaction.
  state_directory: '.',

  // if set to a positive number → max number of concurrent actions
  // if set to a negative number → no limit
  // if not specified or set to 0 → defaults to the number of CPUs on the system
  concurrency: 0,

  // Those commands will be executed in order at start, before everything else
  start: [
    // Create an iptables chain for reaction
    iptables(['-N', 'reaction']),
    // Insert this chain as the first item of the INPUT & FORWARD chains (for incoming connections)
    iptables(['-I', 'INPUT', '-p', 'all', '-j', 'reaction']),
    iptables(['-I', 'FORWARD', '-p', 'all', '-j', 'reaction']),
  ],

  // Those commands will be executed in order at stop, after everything else
  stop: [
    // Remove the chain from the INPUT & FORWARD chains
    iptables(['-D', 'INPUT', '-p', 'all', '-j', 'reaction']),
    iptables(['-D', 'FORWARD', '-p', 'all', '-j', 'reaction']),
    // Empty the chain
    iptables(['-F', 'reaction']),
    // Delete the chain
    iptables(['-X', 'reaction']),
  ],

  // streams are commands
  // they are run and their ouptut is captured
  // *example:* `tail -f /var/log/nginx/access.log`
  // their output will be used by one or more filters
  streams: {
    // streams have a user-defined name
    ssh: {
      // note that if the command is not in environment's `PATH`
      // its full path must be given.
      cmd: ['journalctl', '-n0', '-fu', 'sshd.service'],

      // filters run actions when they match regexes on a stream
      filters: {
        // filters have a user-defined name
        failedlogin: {
          // reaction's regex syntax is defined here: https://docs.rs/regex/latest/regex/#syntax
          regex: [
            // <ip> is predefined in the patterns section
            // ip's regex is inserted in the following regex
            @'authentication failure;.*rhost=<ip>',
            @'Failed password for .* from <ip>',
            @'Invalid user .* from <ip>',
            @'Connection (reset|closed) by (authenticating|invalid) user .* <ip>',
            @'banner exchange: Connection from <ip> port [0-9]*: invalid format',
          ],

          // if retry and retryperiod are defined,
          // the actions will only take place if a same pattern is
          // found `retry` times in a `retryperiod` interval
          retry: 3,
          // format is defined as follows: <integer> <unit>
          // - whitespace between the integer and unit is optional
          // - integer must be positive (>= 0)
          // - unit can be one of:
          //   - ms / millis / millisecond / milliseconds
          //   - s / sec / secs / second / seconds
          //   - m / min / mins / minute / minutes
          //   - h / hour / hours
          //   - d / day / days
          retryperiod: '6h',

          // duplicate specify how to handle matches after an action has already been taken.
          // 3 options are possible:
          // - extend (default): update the pending actions' time, so they run later
          // - ignore: don't do anything, ignore the match
          // - rerun: run the actions again. so we may have the same pending actions multiple times.
          //   (this was the default before 2.2.0)
          // duplicate: extend

          // actions are run by the filter when regexes are matched
          actions: {
            // actions have a user-defined name
            ban: {
              cmd: iptables(['-A', 'reaction', '-s', '<ip>', '-j', 'DROP']),
            },

            unban: {
              cmd: iptables(['-D', 'reaction', '-s', '<ip>', '-j', 'DROP']),
              // if after is defined, the action will not take place immediately, but after a specified duration
              // same format as retryperiod
              after: '2 days',
              // let's say reaction is quitting. does it run all those pending commands which had an `after` duration set?
              // if you want reaction to run those pending commands before exiting, you can set this:
              // onexit: true,
              // (defaults to false)
              // here it is not useful because we will flush and delete the chain containing the bans anyway
              // (with the stop commands)
            },

            mail: {
              cmd: ['sendmail', '...', '<ip>'],
              // some commands, such as alerting commands, are "oneshot".
              // this means they'll be run only once, and won't be executed again when reaction is restarted
              oneshot: true,
            },
          },
          // or use the banFor function defined at the beginning!
          // actions: banFor('48h'),
        },
      },
    },
  },
}

// persistence

// tldr; when an `after` action is set in a filter, such filter acts as a 'jail',
// which is persisted after reboots.

// full;
// when a filter is triggered, there are 2 flows:
//
// if none of its actions have an `after` directive set:
// no action will be replayed.
//
// else (if at least one action has an `after` directive set):
// if reaction stops while `after` actions are pending:
// and reaction starts again while those actions would still be pending:
// reaction executes the past actions (actions without after or with then+after < now)
// and plans the execution of future actions (actions with then+after > now)
