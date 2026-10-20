{
  socket_path: './reaction.sock',
  patterns: {
    num: {
      regex: '[0-9]+',
      ignore: ['1'],
      // ignoreregex: ['2.?'],
    },
  },

  start: [
    ['echo', 'coucou'],
  ],

  stop: [
    ['echo', 'byebye'],
  ],

  streams: {
    s1: {
      cmd: ['sh', '-c', 'seq 20 | while read i; do echo found $((i % 5)); sleep 1; done'],
      filters: {
        f1: {
          regex: [
            '^found [0-9]+$',
          ],
          retry: 4,
          retryperiod: '60s',
          actions: {
            damn: {
              cmd: ['notify-send', 'first stream', 'found!'],
            },
            undamn: {
              cmd: ['notify-send', 'first stream', 'unfound'],
              after: '3s',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
