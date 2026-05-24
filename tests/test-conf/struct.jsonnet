{
  concurrency: 0,
  patterns: {
    ip: { type: 'ip' },
  },
  streams: {
    stream1: {
      cmd: [
        'sh',
        '-c',
        |||
          echo '{"useless_field":"what is the meaning of life","client_ip":"198.51.100.12"}'
          sleep 0.01
          echo '{"useless_field":"what is the meaning of life","client_ip":"198.51.100.13", "allowlisted": true}'
          sleep 0.01
          echo '{"useless_field":"what is the meaning of life","client_ip":"198.51.100.14", "allowlisted": null}'
          sleep 0.01
          echo '{"client_ip":"198.51.100.15"}'
          sleep 0.01
        |||,
      ],
      filters: {
        filter1: {
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
          },
        },
      },
    },
  },
}
