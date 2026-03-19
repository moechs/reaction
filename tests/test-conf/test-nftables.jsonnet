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
    nftables: {
      path: './target/debug/reaction-plugin-nftables',
      check_root: false,
      systemd_options: {
        CapabilityBoundingSet: ['~CAP_NET_ADMIN', '~CAP_PERFMON'],
      },
    },
  },

  streams: {
    s0: {
      cmd: ['bash', '-c', 'sleep 1; for i in $(seq 4); do echo 192.0.2.$i; echo 2001:db8:$i:a31b::$i; sleep 0.1; done; sleep 3'],
      filters: {
        f0: {
          regex: ['^<ip>$'],
          actions: {
            a0: {
              type: 'nftables',
              options: {
                set: 'reactiontest',
                // pattern: 'ip',
                // version: 'ip',
                // chains: ['INPUT', 'FORWARD'],
                // target: 'DROP',
                // action: 'add',
              },
            },
            a1: {
              after: '2s',
              type: 'nftables',
              options: {
                set: 'reactiontest',
                action: 'delete',
              },
            },
          },
        },
      },
    },
  },
}
