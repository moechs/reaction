/* Test that non-utf8 characters are stripped from the commands' output
 *
 * ASCII characters such as \x1b, \x05 are kept as is.
 */
{
  patterns: {
    id: {
      regex: @'.+',
    },
  },
  streams: {
    binary: {
      cmd: ['sh', '-c', 'for n in $(seq 3); do printf "\\n\\x1babc\\xe2 \\x05"; done; printf "\\n"; sleep 0.2'],
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
