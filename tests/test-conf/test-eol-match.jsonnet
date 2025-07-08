{
  patterns: {
    num: { regex: '[0-9]+' },
  },
  streams: {
    s1: {
      cmd: ['sh', '-c', 'for i in $(seq 5); do echo here is $i;  sleep 0.01; done'],
      filters: {
        f1: {
          regex: ['here is <num>$'],
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
