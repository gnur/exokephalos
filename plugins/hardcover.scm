;; Hardcover is an executable Steel plugin. Rust supplies only the sandboxed
;; secret and HTTPS host capabilities; the GraphQL request, response parsing,
;; normalization, and note construction remain here.

(define (xo-plugin-manifest)
  "{\"schema\":1,\"actions\":[{\"id\":\"hardcover-search\",\"description\":\"Search Hardcover\",\"prompt\":\"Book title or author\",\"entrypoint\":\"xo-plugin-run\",\"capabilities\":[\"create-note\",\"network\",\"read-secret\"]}]}")

(define graphql-query
  "query SearchBooks($query: String!, $perPage: Int!, $page: Int!) { search(query: $query, query_type: \"Book\", per_page: $perPage, page: $page) { results } }")

(define (get object key fallback)
  (cond [(not (hash? object)) fallback]
        [(hash-contains? object key) (hash-ref object key)]
        [(hash-contains? object (string->symbol key))
         (hash-ref object (string->symbol key))]
        [else fallback]))

(define (scalar-string value)
  (cond [(string? value) value]
        [(number? value)
         (number->string (if (integer? value) (inexact->exact value) value))]
        [else ""]))

(define (integer-value value)
  (cond [(and (number? value) (integer? value)) (inexact->exact value)]
        [(number? value) value]
        [(string? value) (let ([parsed (string->number value)])
                           (if parsed parsed 0))]
        [else 0]))

(define (first-field object keys)
  (if (null? keys)
      ""
      (let ([value (scalar-string (get object (car keys) ""))])
        (if (string=? value "")
            (first-field object (cdr keys))
            value))))

(define (authors object)
  (let ([values (get object "author_names" (get object "authors" '()))])
    (if (list? values)
        (filter (lambda (value) (not (string=? value "")))
                (map (lambda (value)
                       (if (hash? value)
                           (first-field value '("name" "title"))
                           (scalar-string value)))
                     values))
        (let ([value (scalar-string values)])
          (if (string=? value "") '() (list value))))))

(define (book-series object)
  (let* ([featured (get object "featured_series" #f)]
         [series (if (hash? featured) (get featured "series" #f) #f)]
         [name (if (hash? series) (first-field series '("name")) "")]
         [position (if (hash? featured) (scalar-string (get featured "position" "")) "")])
    (cond [(and (not (string=? name "")) (not (string=? position "")))
           (string-append name ", #" position)]
          [(not (string=? name "")) name]
          [else
           (let ([names (get object "series_names" '())])
             (if (and (list? names) (not (null? names)))
                 (scalar-string (car names))
                 ""))])))

(define (book-url object)
  (let ([direct (first-field object '("goodreads_url" "url" "canonical_url"))])
    (if (not (string=? direct ""))
        direct
        (let* ([external (get object "external_ids" #f)]
               [goodreads (if (hash? external)
                              (first-field external '("goodreads" "goodreads_id" "goodreadsId"))
                              (first-field object '("goodreads_id" "goodreadsId")))]
               [slug (first-field object '("slug"))]
               [id (first-field object '("id"))])
          (cond [(not (string=? goodreads ""))
                 (string-append "https://www.goodreads.com/book/show/" goodreads)]
                [(not (string=? slug ""))
                 (string-append "https://hardcover.app/books/" slug)]
                [(not (string=? id ""))
                 (string-append "https://hardcover.app/books/" id)]
                [else ""])))))

(define (result-documents results)
  (if (list? results)
      results
      (let ([hits (get results "hits" '())])
        (if (list? hits)
            (map (lambda (hit) (get hit "document" (hash))) hits)
            '()))))

(define (take-up-to values count)
  (if (or (= count 0) (null? values))
      '()
      (cons (car values) (take-up-to (cdr values) (- count 1)))))

(define (book-choice book)
  (let* ([title (first-field book '("title"))]
         [series (book-series book)]
         [display-title (if (string=? series "")
                            title
                            (string-append title " (" series ")"))]
         [book-authors (authors book)]
         [label (if (null? book-authors)
                    display-title
                    (string-append display-title " — " (string-join book-authors ", ")))]
         [frontmatter
           (hash "type" "book"
                 "title" display-title
                 "tags" '("to-read")
                 "author" book-authors
                 "pages" (integer-value (get book "pages" (get book "page_count" 0)))
                 "cover" (first-field book '("image" "image_url" "cover" "cover_url"))
                 "url" (book-url book)
                 "isbn" (first-field book '("isbn_13" "isbn13" "isbn" "isbn_10" "isbn10"))
                 "year" (first-field book '("release_year"))
                 "series" series)])
    (hash "label" label
          "note" (hash "frontmatter" frontmatter
                       "body" (first-field book '("description"))))))

(define (xo-plugin-run input)
  (let* ([request
           (value->jsexpr-string
             (hash "query" graphql-query
                   "variables" (hash "query" input "perPage" 5 "page" 1)))]
         [headers
           (value->jsexpr-string
             (hash "Authorization"
                   (string-append "Bearer " (xo-secret "HARDCOVER_TOKEN"))
                   "Content-Type" "application/json"
                   "User-Agent" "exokephalos-steel/1.0"))]
         [response
           (string->jsexpr
             (xo-http-post-json "https://api.hardcover.app/v1/graphql"
                                headers
                                request))]
         [data (get response "data" (hash))]
         [search (get data "search" (hash))]
         [books (take-up-to (result-documents (get search "results" '())) 5)])
    (value->jsexpr-string
      (hash "choices"
            (filter (lambda (choice)
                      (not (string=? (get choice "label" "") "")))
                    (map book-choice books))))))
