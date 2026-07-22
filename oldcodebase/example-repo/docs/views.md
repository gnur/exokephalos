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
                 :subviews [{:name "All" :when (fn [_] true)}
                            {:name "Todo" :when (fn [note] (= note.status "todo"))}]}}
 :actions {}}
```

Views and subviews use Fennel predicates, not CEL. A predicate receives a flat note table: frontmatter fields such as `:id`, `:type`, `:tags`, and custom fields are direct keys, alongside `:path` and `:body`; there is no `:frontmatter` key. CEL remains available only for API-key restrictions and `POST /api/query/ids`.

Each view requires `:name`, `:key`, and `:when`. `:title-field`, `:sort-field`, and `:sort-order` default to `"title"`, `"created"`, and `"desc"`; a missing subview list receives an `All` subview. xo also supplies the built-in `All` view on key `0`.
