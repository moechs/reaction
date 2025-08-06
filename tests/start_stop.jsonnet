local echo(message) = ['sh', '-c', 'echo %s >> ./log' % message];
{
  patterns: {
    num: {
      regex: '[0-9]+',
    },
  },

  start: [
    echo('start 1'),
    echo('start 2'),
  ],

  stop: [
    echo('stop 1'),
    echo('stop 2'),
  ],

  streams: {
    s1: {
      cmd: ['sh', '-c', 'seq 2 | while read i; do echo runtime $i; sleep 0.1; done'],
      filters: {
        f1: {
          regex: [
            '^runtime <num>$',
          ],
          actions: {
            one: {
              cmd: echo('runtime <num>'),
            },
            two: {
              cmd: echo('after <num>'),
              after: '1s',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
