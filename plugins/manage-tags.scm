;; Host-provided interactive tag manager.
;; Install with: xo plugin install manage-tags plugins/manage-tags.scm
;; xo supplies the native multi-selection and tag-picker UI; the plugin does not
;; receive filesystem or terminal access.

(define (xo-plugin-manifest)
  "{\"schema\":1,\"actions\":[{\"id\":\"manage-tags\",\"description\":\"Manage tags on selected notes\",\"interaction\":\"tag-picker\",\"capabilities\":[\"mutate-note\"]}]}")
