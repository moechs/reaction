{
  runtime_directory: '.',
  state_directory: '.',
  patterns: {
    num: { regex: '[0-9]+' },
  },
  streams: {
    s1: {
      cmd: ['sh', '-c', 'sleep 1'],
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
