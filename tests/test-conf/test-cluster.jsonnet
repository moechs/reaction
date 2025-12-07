{
  patterns: {
    num: {
      regex: @"[0-9]+",
    },
    all: {
      regex: @".*",
    },
  },

  plugins: {
    cluster: {
      path: './target/debug/reaction-plugin-cluster',
      check_root: false,
      systemd_options: {
        DynamicUser: ['false'],
      },
    },
  },

  streams: {
    s0: {
      cmd: ['bash', '-c', 'sleep 1; for i in $(seq 4); do echo $i; sleep 0.1; done'],
      filters: {
        f0: {
          regex: ['^<num>$'],
          actions: {
            a0: {
              type: 'cluster_send',
              options: {
                send: 'NODE a0 <num>',
                to: 's1',
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
