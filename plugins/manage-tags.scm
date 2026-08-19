;; Host-provided interactive tag manager.
;; Install this plugin through Steel Forge into ~/.config/xo/plugins/.
;; xo supplies the native multi-selection and tag-picker UI; the plugin does not
;; receive filesystem or terminal access.

(define (xo-plugin-manifest)
  "{\"schema\":1,\"actions\":[{\"id\":\"manage-tags\",\"description\":\"Manage tags on selected notes\",\"interaction\":\"tag-picker\",\"capabilities\":[\"mutate-note\"]}]}")
