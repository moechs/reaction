/* Test that non-utf8 characters are stripped from the commands' output
 *
 * ASCII characters such as \x1b, \x05 are kept as is.
 */
{
  runtime_directory: '.',
  state_directory: '.',
  patterns: {
    id: {
      regex: @'.+',
    },
  },
  streams: {
    binary: {
      cmd: ['sh', '-c', 'for n in $(seq 3); do printf "\\n\\033abc\\342 \\005"; done; printf "\\n"; sleep 0.2'],
      eof: {
        behavior: 'exit',
      },
      filters: {
        filt1: {
          regex: [
            @'<id>',
          ],
          actions: {
            act: {
              cmd: ['sh', '-c', 'echo \'received "<id>"\' >>log'],
            },
          },
        },
      },
    },
  },
}
