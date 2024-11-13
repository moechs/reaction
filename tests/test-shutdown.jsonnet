{
  patterns: {
    zero: {
      regex: @'0',
    },
  },

  streams: {
    idle: {
      cmd: ['sh', '-c', 'while true; do sleep 1; done'],
      filters: {
        filt1: {
          regex: [
            @'abc',
          ],
          actions: {
            act: {
              cmd: ['echo', '1'],
            },
          },
        },
      },
    },
  },
}

