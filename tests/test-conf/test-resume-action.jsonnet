{
  patterns: {
    ip: {
      type: 'ip',
      ipv6mask: 64,
    },
  },
  start: [
    ['sh', '-c', 'echo starting >>./log'],
  ],
  stop: [
    ['sh', '-c', 'echo stopping >>./log'],
  ],
  streams: {
    s1: {
      cmd: ['sh', '-c', 'sleep 2; echo T 10.1.0.1; sleep 0.2; echo T 10.1.0.1; sleep 0.8'],
      eof: {
        behavior: 'exit',
      },
      filters: {
        f1: {
          regex: ['T <ip>'],
          actions: {
            log_start4: {
              ipv4only: true,
              cmd: ['sh', '-c', 'echo start4 <ip> >>./log'],
            },
            log_start6: {
              ipv6only: true,
              cmd: ['sh', '-c', 'echo start6 <ip> >>./log'],
            },
            log_stop4: {
              ipv4only: true,
              onexit: false,
              after: '2s',
              cmd: ['sh', '-c', 'echo stop4 <ip> >> ./log'],
            },
            log_stop6: {
              ipv6only: true,
              onexit: false,
              after: '2s',
              cmd: ['sh', '-c', 'echo stop6 <ip> >> ./log'],
            },
          },
        },
      },
    },
  },
}
