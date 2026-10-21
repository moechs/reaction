{
  log_level: 'debug',
  runtime_directory: '.',
  state_directory: '.',
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
      cmd: ['sh', '-c', 'seq 20 | while read i; do echo found $((i % 5)); sleep 1; done'],
      filters: {
        f1: {
          regex: [
            '^found <num>$',
          ],
          retry: 2,
          retryperiod: '60s',
          actions: {
            damn: {
              cmd: ['sh', '-c', 'notify-send "$myvar" "ban <num>"'],
              env: {
                myvar: 'DAMN',
              },
            },
            undamn: {
              cmd: ['sh', '-c', 'notify-send "$myvar" "unban <num>"'],
              env: {
                myvar: 'UNDAMN',
              },
              after: '3s',
              onexit: true,
            },
          },
        },
        f2: {
          regex: [
            "^can't found <num>$",
          ],
          retry: 2,
          retryperiod: '60s',
          actions: {
            damn: {
              cmd: ['notify-send', 'you should not see that', 'ban <num>'],
            },
            undamn: {
              cmd: ['notify-send', 'you should not see that', 'unban <num>'],
              after: '3s',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
