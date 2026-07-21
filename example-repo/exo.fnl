{:default-view :notes
 :views {:notes {:name "Notes"
                 :key "n"
                 :show-tags true
                 :when (fn [note] (= note.type "note"))
                 :template "---\ntype: note\ntags: []\nid: {{.ID}}\ncreated: {{.Date}}\ntitle: \"{{.Title}}\"\n---\n\n# {{.Title}}\n"
                 :subviews [{:name "All" :when (fn [_] true)}]}}
 :actions {}}
