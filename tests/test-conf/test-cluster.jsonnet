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
      path: "./target/debug/reaction-plugin-cluster",
      check_root: false,
      systemd_options: {
        DynamicUser: ["false"],
      },
      options: {
        clusters: {
          org1: {
            listen_port: 9000,
            bootstrap_nodes: {
              "public_key": ["127.0.0.1:9001"],
            },
          },
        },
      },
    },
  },

  streams: {
    s0: {
      cmd: ["bash", "-c", "for i in $(seq 4); do echo $i; sleep 0.1; done; sleep 1.2"],
      filters: {
        f0: {
          regex: ["^<num>$"],
          actions: {
            a0: {
              type: "virtual",
              options: {
                send: "a0 <num>",
                to: "s1",
              }
            },
            b0: {
              type: "virtual",
              options: {
                send: "b0 <num>",
                to: "s1",
              },
              after: "600ms",
            },
          },
        },
      },
    },
    s1: {
      type: "cluster",
      options: {},
      filters: {
        f1: {
          regex: ["^<all>$"],
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
