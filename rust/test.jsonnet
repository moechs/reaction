{
  patterns: {
    num: {
      regex: '[0-9]+',
      ignore: ['1'],
      // ignoreregex: ['2.?'],
    },
    letter: {
      regex: '[a-z]+',
      ignore: ['b'],
      // ignoreregex: ['b.?'],
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
      cmd: ['sh', '-c', "seq 20 | tr ' ' '\n' | while read i; do echo found $((i % 5)); sleep 0.3; done"],
      filters: {
        f1: {
          regex: [
            '^found <num>$',
          ],
          retry: 2,
          retryperiod: '5s',
          actions: {
            damn: {
              cmd: ['notify-send', 'first stream', 'ban <num>'],
            },
            undamn: {
              cmd: ['notify-send', 'first stream', 'unban <num>'],
              after: '6s',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
