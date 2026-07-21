---
type: doc
tags: []
id: viewsdoc
created: 2026-07-09
title: "View Configurations"
---

# View Configurations

exo reads one synced workspace entrypoint, `exo.fnl`, plus optional Fennel or Lua modules below `modules/`. `.exo/` is local-only and holds `tui.fnl`, `serve.fnl`, permissions, keys, and databases.

```fennel
{:default-view :notes
 :views {:notes {:name "Notes"
                 :key "n"
                 :show-tags true
                 :when (fn [note] (= note.type "note"))
                 :template "---\ntype: note\ntags: []\ntitle: {{.Title}}\n---\n"
                 :subviews [{:name "All" :when (fn [_] true)}
                            {:name "Todo" :when (fn [note] (= note.frontmatter.status "todo"))}]}}
 :actions {}}
```

Views and subviews use Fennel predicates, not CEL. A predicate receives `:id`, `:path`, `:type`, `:tags`, `:frontmatter`, and `:body`, and must return a boolean. CEL remains available only for API-key restrictions and `POST /api/query/ids`.

Each view requires `:name`, `:key`, `:when`, and `:template`. `:title-field`, `:sort-field`, and `:sort-order` default to `"title"`, `"created"`, and `"desc"`; a missing subview list receives an `All` subview. xo also supplies the built-in `All` view on key `0`.
