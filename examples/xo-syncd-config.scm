; xo-syncd server configuration; command-line flags override these values.
(xo-syncd-config
  (schema 1)
  (state-dir "/var/lib/xo-syncd")
  (bind "127.0.0.1:9464")
  (oidc-issuer "https://id.example.com")
  (oidc-audience "https://notes.example.com")
  (oidc-client-id "YOUR_PUBLIC_CLIENT_ID"))
