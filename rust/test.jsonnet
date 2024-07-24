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
    tailDown1: {
      cmd: ['sh', '-c', "echo 1_abc 2_abc 3_abc abc_1 abc_2 abc_3 | tr ' ' '\n' | while read i; do sleep 1; echo found $i; done; sleep 2"],
      filters: {
        findIP: {
          regex: [
            '^found <num>_<letter>$',
            '^found <letter>_<num>$',
          ],
          retry: 2,
          retryperiod: '30s',
          actions: {
            damn: {
              cmd: ['echo', '<num>'],
            },
            undamn: {
              cmd: ['echo', 'undamn', '<num>'],
              after: '28s',
              onexit: true,
            },
          },
        },
      },
    },
    tailDown2: {
      cmd: ['sh', '-c', "echo 1_abc 2_abc 3_abc abc_1 abc_2 abc_3 | tr ' ' '\n' | while read i; do sleep 1; echo found $i; done; sleep 3"],
      filters: {
        findIP: {
          regex: [
            '^found <num>_<letter>$',
            '^found <letter>_<num>$',
          ],
          retry: 2,
          retryperiod: '30s',
          actions: {
            damn: {
              cmd: ['echo', '<num>'],
            },
            undamn: {
              cmd: ['echo', 'undamn', '<num>'],
              after: '28s',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
