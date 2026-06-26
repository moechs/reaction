{
  patterns: {
    ip: {
      type: 'ip',
    },
  },

  plugins: {
    ipset: {
      path: './target/debug/reaction-plugin-httpban',
      check_root: false,
      systemd: false,
      options: {
        backends: {
          funkwhale: {
            host: 'http://localhost:8999',
          },
          vault: {
            host: 'http://localhost:8998',
          },
          iocaine: {
            host: 'unix:///run/iocaine/iocaine.sock',
          },
        },
        domains: {
          'music.ppom.me': {
            case: [
              {
                set: 'pubapps',
                code: 404,
                page: '/var/lib/reaction/404.html',
                // inline: 'Not found uwu',
              },
            ],
            default: {
              backend: 'funkwhale',
            },
          },
          'ppom.me/vault': {
            case: [
              {
                set: 'privapps',
                code: 403,
                page: '/var/lib/reaction/403.html',
              },
            ],
            default: {
              backend: 'vault',
            },
          },
          'tokipona.ppom.me': {
            case: {
              set: 'pubsites',
              backend: 'iocaine',
            },
            default: {
              backend: 'nginx',
            },
          },
        },
      },
    },
  },

  streams: {
    s0: {
      cmd: ['bash', '-c', @'sleep 1; for i in $(seq 4); do echo 192.0.2.$i; echo 2001:db8:$i:a31b::$i; sleep 0.1; done; sleep 3'],
      eof: {
        behavior: 'exit',
      },
      filters: {
        f0: {
          regex: ['^<ip>$'],
          actions: {
            a0: {
              type: 'httpban',
              options: {
                add: ['ppom'],
              },
            },
            a1: {
              after: '2s',
              type: 'httpban',
              options: {
                remove: ['ppom'],
              },
            },
          },
        },
      },
    },
  },
}
