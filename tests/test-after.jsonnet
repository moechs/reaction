local log(cat) = [
  'sh', '-c', 'echo "' + cat + ' <id>" >>log',
];
{
  patterns: {
    id: {
      regex: @'\d+',
    },
  },
  streams: {
    idle: {
      cmd: ['sh', '-c', 'for n in 1 1 3 2 3 1 2 2 3; do echo $n; done; sleep 2'],
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

