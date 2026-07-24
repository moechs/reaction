{
  patterns: {
    ip: {
      type: 'ip',
    },
  },

  plugins: {
    httpban: {
      path: './target/debug/reaction-plugin-httpban',
      check_root: false,
      systemd: false,
      options: {
        // les backends, un peu comme dans nginx
        backends: {
          funkwhale: {
            host: 'http://localhost:8999',
          },
          vault: {
            host: 'http://localhost:8998',
          },
          iocaine: {
            host: 'unix:///run/iocaine/iocaine.sock',
          },
          nginx: {
            host: 'unix:///run/nginx/nginx.sock',
          },
        },
        // la conf pour chaque domaine
        domains: {
          'music.ppom.me': {
            case: [
              // si l'ip de la requête est dans le set 'pubapps', alors on renvoie tel fichier html en 404
              {
                set: 'pubapps',
                code: 404,
                page: '/var/lib/reaction/404.html',
              },
            ],
            // sinon on renvoie vers la webapp funkwhale
            default: {
              backend: 'funkwhale',
            },
          },
          // ça c'est plus spécifique que l'entrée suivante du coup c'est prioritaire
          'ppom.me/vault': {
            case: [
              // si l'ip de la requête est dans le set 'privapps', alors on renvoie tel texte inline en 404
              {
                set: 'privapps',
                code: 403,
                inline: 'Unauthorized uwu',
              },
              // si l'ip de la requête est dans le set 'pubapps', alors on renvoie vers iocaine
              {
                set: 'pubapps',
                backend: 'iocaine',
              },
            ],
            // sinon on renvoie vers la webapp vault
            default: {
              backend: 'vault',
            },
          },
          'ppom.me': {
            // jamais de ban, toujours le comportement par défaut
            default: {
              backend: 'nginx',
            },
          },
          'tokipona.ppom.me': {
            // quand on a qu'un cas, on peut simplifier l'array d'objects en juste un object
            case: {
              set: 'pubsites',
              backend: 'iocaine',
            },
            default: {
              backend: 'nginx',
            },
          },
        },
      },
    },
  },

  streams: {
    s0: {
      cmd: ['bash', '-c', @'sleep 1; for i in $(seq 4); do echo 192.0.2.$i; echo 2001:db8:$i:a31b::$i; sleep 0.1; done; sleep 3'],
      eof: {
        behavior: 'exit',
      },
      filters: {
        f0: {
          regex: ['^<ip>$'],
          actions: {
            a0: {
              // l'action c'est d'ajouter l'IP à tel set.
              type: 'httpban',
              options: {
                add: ['pubapps'],
              },
            },
            a1: {
              after: '2s',
              // l'action c'est de retirer l'IP de tel set.
              type: 'httpban',
              options: {
                remove: ['pubapps'],
              },
            },
          },
        },
      },
    },
  },
}
