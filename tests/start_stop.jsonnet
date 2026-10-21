local echo(message, before='true') = [
  'sh',
  '-c',
  before + '; echo ' + message + ' >> ./log',
];
{
  state_directory: '.',
  runtime_directory: '.',

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
      eof: {
        behavior: 'ignore',
      },
      filters: {
        f1: {
          duplicate: 'rerun',
          regex: [
            '^runtime <num>$',
          ],
          actions: {
            one: {
              cmd: echo('runtime <num>'),
            },
            two: {
              cmd: echo('after', before='sleep 0.2'),
              after: '5m',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
