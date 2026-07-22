{:default-view :notes
 :views {:notes {:name "Notes" :key "n" :show-tags true
                 :when (fn [note] (= note.type "note"))}}
 :actions {:mark-done {:description "Mark done"
                       :when (fn [note] (has-tag note.tags "todo"))
                       :run (fn [note] (assoc note :tags (add-tag note.tags "done")))}}}
