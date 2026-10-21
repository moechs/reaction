{
  runtime_directory: '.',
  state_directory: '.',
  concurrency: 1,
  patterns: {
    num: { regex: '[0-9]+' },
  },
  streams: {
    s1: {
      cmd: ['sh', '-c', 'for i in $(seq 5); do echo here is $i; echo here is $i 1>&2; sleep 0.01; done'],
      eof: {
        behavior: 'exit',
      },
      filters: {
        f1: {
          regex: ['here is <num>'],
          retry: 2,
          retryperiod: '2s',
          actions: {
            log: {
              cmd: ['sh', '-c', 'echo <num> >>./log'],
            },
          },
        },
      },
    },
  },
}
