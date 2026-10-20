{
  socket_path: './reaction.sock',
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
          type: 'struct',
          options: {
            fields: {
              test: '<num>',
            },
          },
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
      },
    },
  },
}
