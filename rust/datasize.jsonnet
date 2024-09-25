{
  patterns: {
    num: {
      regex: @'([0-9]+)',
    },
  },
  streams: {
    s1: {
      cmd: ['seq', '-w', '499999'],
      filters: {
        f1: {
          regex: [
            '^<num>$',
          ],
          retry: 10,
          retryperiod: '1m',
          actions: {
            a: {
              cmd: ['true'],
            },
            b: {
              cmd: ['true'],
              after: '1m',
            },
          },
        },
      },
    },
  },
}
