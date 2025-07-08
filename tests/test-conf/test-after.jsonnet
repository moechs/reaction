local log(cat) = [
  // NOTE: do not log the ID as it would be out of order since all cat commands
  // are executed at once
  'sh', '-c', 'echo "' + cat + '" >>log',
];
{
  patterns: {
    id: {
      regex: @'\d+',
    },
  },
  streams: {
    idle: {
      cmd: ['sh', '-c', 'for n in $(seq 9); do echo $n; done; sleep 2'],
      filters: {
        filt1: {
          regex: [
            @'<id>',
          ],
          actions: {
            act: {
              cmd: log('im'),
            },
            delayed: {
              cmd: log('de'),
              after: '1s',
            },
            longafter: {
              cmd: log('la'),
              after: '1d',
              onexit: true,
            },
          },
        },
      },
    },
  },
}
