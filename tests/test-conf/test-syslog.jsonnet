{
  state_directory: '.',
  runtime_directory: '.',

  patterns: {
    ip: {
      type: 'ipv4',
    },
  },

  plugins: {
    syslog: {
      path: './target/debug/reaction-plugin-syslog',
      check_root: false,
      systemd: false,
      options: {
        listen: '127.0.0.1:5140',
        protocol: 'udp',
        parse: 'auto',
        reconnect_delay: '1s',
      },
    },
  },

  streams: {
    auth: {
      type: 'syslog',
      options: {
        select: {
          facilities: ['auth', 'authpriv'],
          severities: ['err', 'crit', 'alert', 'emerg'],
        },
        prefix: '{host} {appname}[{procid}]: ',
      },
      filters: {
        failedlogin: {
          regex: ['Failed password for .* from <ip>'],
          retry: 3,
          retryperiod: '10m',
          actions: {
            log: {
              cmd: ['true'],
            },
          },
        },
      },
    },
  },
}
