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
                pattern: 'ip',
                version: 46,
                chains: ['INPUT', 'FORWARD'],
              },
            },
            b0: {
              type: 'cluster_send',
              options: {
                send: 'NODE b0 <num>',
                to: 's1',
              },
              after: '1s',
            },
          },
        },
      },
    },
    s1: {
      type: 'cluster',
      options: {
        listen_port: 1234,
        bind_ipv4: '127.0.0.1',
        bind_ipv6: null,
        message_timeout: '30s',
        nodes: [{
          public_key: 'PUBLIC_KEY',
          addresses: ['127.0.0.1:4321'],
        }],
      },
      filters: {
        f1: {
          regex: ['^<all>$'],
          actions: {
            a1: {
              cmd: ['sh', '-c', 'echo <all> >>./log'],
            },
          },
        },
      },
    },
  },
}
