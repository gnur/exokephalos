(local has-tag (fn [tags tag]
  (var found false)
  (each [_ value (ipairs tags)]
    (when (= value tag) (set found true)))
  found))

(local replace-tag (fn [tags remove add]
  (local result [])
  (each [_ value (ipairs tags)]
    (when (not= value remove) (table.insert result value)))
  (when (not (has-tag result add)) (table.insert result add))
  result))

{:default-view :notes
 :views
 {:books {:name "Books" :key "b" :show-tags false
          :title-field "title" :subtitle-field "author" :sort-field "added" :sort-order "desc"
          :when (fn [note] (or (has-tag note.tags "read") (has-tag note.tags "to-read") (has-tag note.tags "reading") (has-tag note.tags "stopped-reading")))
          :stats-template "books/stats"
          :subviews [{:name "All" :when (fn [_] true)} {:name "To Read" :when (fn [note] (has-tag note.tags "to-read"))} {:name "Reading" :when (fn [note] (has-tag note.tags "reading"))} {:name "Read" :when (fn [note] (has-tag note.tags "read"))}]}
  :docs {:name "Docs" :key "d" :show-tags true :title-field "title" :sort-field "title" :sort-order "asc"
         :when (fn [note] (= note.type "doc"))
         :subviews [{:name "All" :when (fn [_] true)}]}
  :notes {:name "Notes" :key "n" :show-tags true :title-field "title" :sort-field "created" :sort-order "desc"
          :when (fn [note] (and (= note.type "note") (not (or (has-tag note.tags "read") (has-tag note.tags "to-read") (has-tag note.tags "reading") (has-tag note.tags "stopped-reading")))))
          :subviews [{:name "All" :when (fn [_] true)} {:name "Todo" :when (fn [note] (and (has-tag note.tags "todo") (not (has-tag note.tags "done"))))} {:name "Recipes" :when (fn [note] (has-tag note.tags "recept"))}]}
  :secrets {:name "Secrets" :key "s" :show-tags false :title-field "name" :sort-field "created" :sort-order "desc"
            :when (fn [note] (= note.type "secret"))
            :subviews [{:name "all" :when (fn [_] true)} {:name "acceptance" :when (fn [note] (has-tag note.tags "acc"))} {:name "production" :when (fn [note] (has-tag note.tags "prod"))}]}
  :webhooks {:name "Webhooks" :key "w" :show-tags false :title-field "source" :subtitle-field "type" :sort-field "timestamp" :sort-order "desc"
             :when (fn [note] (or (= note.type "webhook") (= note.type "alert")))
             :subviews [{:name "All" :when (fn [_] true)}]}}
 :actions
 {:finish-book {:description "Mark book as finished reading" :when (fn [note] (has-tag note.tags "reading")) :run (fn [note] (assoc note :frontmatter (assoc note.frontmatter :tags (replace-tag note.tags "reading" "read"))))}
  :start-book {:description "Start reading this book" :when (fn [note] (has-tag note.tags "to-read")) :run (fn [note] (assoc note :frontmatter (assoc note.frontmatter :tags (replace-tag note.tags "to-read" "reading"))))}
  :mark-done {:description "Mark item as done" :when (fn [note] (and (has-tag note.tags "todo") (not (has-tag note.tags "done")))) :run (fn [note] (assoc note :frontmatter (assoc note.frontmatter :tags (replace-tag note.tags "" "done"))))}}}
