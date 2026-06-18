// build a small shell program that intercepts SIGTERM and either exit or ignores it
local build_cmd(log, exit=true) = [
  'sh',
  '-c',
  |||
    t() {
      echo sigterm >>%s
      %s
    }
    trap t SIGTERM
    while true; do sleep 1; done
  |||
  % [log, if exit then 'exit 0' else 'return'],
];
local dummy_filter = {
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
};
{
  patterns: {
    zero: {
      regex: @'0',
    },
  },

  streams: {
    exit_on_sigint: {
      cmd: build_cmd('log_term', exit=true),
      filters: dummy_filter,
      eof: {
        behavior: 'exit',
      },
    },
    exit_on_sigkill: {
      cmd: build_cmd('log_kill', exit=false),
      filters: dummy_filter,
      eof: {
        behavior: 'exit',
      },
    },
  },
}
