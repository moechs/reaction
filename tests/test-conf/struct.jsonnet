{
  state_directory: '.',
  runtime_directory: '.',
  concurrency: 0,
  patterns: {
    ip: {
      type: 'ip',
      ignore: ['198.51.100.11'],
      ignorecidr: ['198.51.100.2/31'],
    },
  },
  streams: {
    stream1: {
      cmd: [
        'sh',
        '-c',
        |||
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.2"}'
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.3"}'
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.11"}'
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.12"}'
          sleep 0.01
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.13", "allowlisted": true}'
          sleep 0.01
          echo '{"useless_field":"what is the meaning of life","n":12,"f":34.56,"client_ip":"198.51.100.14","allowlisted":false}'
          echo '{"f":34.56}'
          sleep 0.01
          echo '{"client_ip":"198.51.100.15"}'
          sleep 0.01
        |||,
      ],
      eof: {
        behavior: 'ignore',
      },
      filters: {
        filter1: {
          duplicate: 'rerun',
          type: 'struct',
          options: {
            lang: 'json',
            fields: {
              client_ip: {
                regex: '<ip>',
              },
              allowlisted: {
                exists: false,
              },
            },
          },
          actions: {
            // Don't mix code and data at home!
            // You may permit arbitrary execution from vilains,
            // if your regex is permissive enough.
            // This is OK only for testing purposes.
            action1: {
              cmd: ['sh', '-c', 'echo <ip> >> ./out.txt'],
            },
            // This action is just here to enable persistance on the filter
            action2: {
              cmd: ['true'],
              after: '1d',
            },
          },
        },
        filter2: {
          type: 'struct',
          options: {
            lang: 'json',
            fields: {
              client_ip: {
                regex: '<ip>',
              },
            },
          },
          actions: {
            // Don't mix code and data at home!
            // You may permit arbitrary execution from vilains,
            // if your regex is permissive enough.
            // This is OK only for testing purposes.
            action1: {
              cmd: ['sh', '-c', 'echo <ip> >> ./out2.txt'],
            },
          },
        },
      },
    },
  },
}
