;; Example xo workspace configuration translated from the former Fennel setup.
;; Current workspace behavior is declarative Steel: predicates and effects are
;; data, and mutating actions require explicit capability grants.
;;
;; The former books/stats template has no current view-descriptor equivalent,
;; so it is intentionally omitted. The reading actions use the execution-time
;; (now) value supplied by the host to write deterministic RFC 3339 timestamps.

(workspace-config
  (schema 1)
  (default-view "notes")
  (query-limit 500)
  (views
    (view
      (id "notes")
      (name "Notes")
      (key "n")
      (show-tags #t)
      (title-field "title")
      (subtitle-field #f)
      (sort-field "created")
      (descending #t)
      (preview #f)
      (predicate
        (all
          (field-equals "type" "note")
          (not
            (any
              (has-tag "read")
              (has-tag "to-read")
              (has-tag "reading")
              (has-tag "stopped-reading")))))
      (subviews
        (subview
          (id "all")
          (name "All")
          (predicate (always)))
        (subview
          (id "recipes")
          (name "Recipes")
          (predicate (has-tag "recept")))
        (subview
          (id "todo")
          (name "Todo")
          (predicate
            (all
              (has-tag "todo")
              (not (has-tag "done")))))))
    (view
      (id "books")
      (name "Books")
      (key "b")
      (show-tags #f)
      (title-field "title")
      (subtitle-field "author")
      (sort-field "added")
      (descending #t)
      (preview #f)
      (predicate
        (any
          (has-tag "read")
          (has-tag "to-read")
          (has-tag "reading")
          (has-tag "stopped-reading")))
      (subviews
        (subview
          (id "all")
          (name "All")
          (predicate (always)))
        (subview
          (id "to-read")
          (name "To Read")
          (predicate (has-tag "to-read")))
        (subview
          (id "reading")
          (name "Reading")
          (predicate (has-tag "reading")))
        (subview
          (id "read")
          (name "Read")
          (predicate (has-tag "read")))))
    (view
      (id "webhooks")
      (name "Webhooks")
      (key "w")
      (show-tags #f)
      (title-field "source")
      (subtitle-field "type")
      (sort-field "timestamp")
      (descending #t)
      (preview #f)
      (predicate
        (any
          (field-equals "type" "webhook")
          (field-equals "type" "alert")))
      (subviews
        (subview
          (id "all")
          (name "All")
          (predicate (always)))))
    (view
      (id "secrets")
      (name "Secrets")
      (key "s")
      (show-tags #f)
      (title-field "name")
      (subtitle-field #f)
      (sort-field "created")
      (descending #t)
      (preview #f)
      (predicate (field-equals "type" "secret"))
      (subviews
        (subview
          (id "acceptance")
          (name "Acceptance")
          (predicate (has-tag "acc")))
        (subview
          (id "production")
          (name "Production")
          (predicate (has-tag "prod"))))))
  (actions
    (action
      (id "finish-book")
      (description "Mark book as finished reading")
      (predicate (has-tag "reading"))
      (effects
        (remove-tag "reading")
        (add-tag "read")
        (set-field "finished" (now))))
    (action
      (id "start-book")
      (description "Start reading this book")
      (predicate (has-tag "to-read"))
      (effects
        (remove-tag "to-read")
        (add-tag "reading")
        (set-field "started" (now))))
    (action
      (id "mark-done")
      (description "Mark item as done")
      (predicate
        (all
          (has-tag "todo")
          (not (has-tag "done"))))
      (effects
        (add-tag "done"))))
  (templates)
  (capability-grants
    (grant
      (action "finish-book")
      (capabilities mutate-note))
    (grant
      (action "start-book")
      (capabilities mutate-note))
    (grant
      (action "mark-done")
      (capabilities mutate-note))))
