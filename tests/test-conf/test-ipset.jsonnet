{
  patterns: {
    ip: {
      type: 'ip',
    },
    all: {
      regex: @".*",
    },
  },

  plugins: {
    cluster: {
      path: './target/debug/reaction-plugin-ipset',
      check_root: false,
      systemd_options: {
        DynamicUser: ['false'],
      },
    },
  },

  streams: {
    s0: {
      cmd: ['bash', '-c', 'sleep 1; for i in $(seq 4); do echo 192.0.2.$i; sleep 0.1; done'],
      filters: {
        f0: {
          regex: ['^<ip>$'],
          actions: {
            a0: {
              type: 'ipset',
              options: {
                set: 'reactiontest',
                // pattern: 'ip',
                // version: 46,
                // chains: ['INPUT', 'FORWARD'],
                // target: 'DROP',
                // action: 'add',
              },
            },
            a1: {
              after: '10s',
              type: 'ipset',
              options: {
                set: 'reactiontest',
                action: 'del',
              },
            },
          },
        },
      },
    },
  },
}
