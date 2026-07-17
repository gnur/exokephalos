package hardcover

import "testing"

func TestExtractBooksFromSearchResultsList(t *testing.T) {
	data := map[string]interface{}{
		"data": map[string]interface{}{
			"search": map[string]interface{}{
				"results": []interface{}{
					map[string]interface{}{
						"title":        "Genesis (First Colony, #1)",
						"author_names": []interface{}{"Ken Lozito"},
						"description":  "Humanity's first colony faces an unexpected threat.",
						"pages":        float64(328),
						"image":        "https://example.com/cover.jpg",
						"external_ids": map[string]interface{}{
							"goodreads": "36284236-genesis",
						},
						"release_year": float64(2017),
					},
				},
			},
		},
	}

	books := extractBooks(data, 5)
	if len(books) != 1 {
		t.Fatalf("expected 1 book, got %d", len(books))
	}
	book := books[0]
	if book.Title != "Genesis (First Colony, #1)" {
		t.Fatalf("title = %q", book.Title)
	}
	if len(book.Authors) != 1 || book.Authors[0] != "Ken Lozito" {
		t.Fatalf("authors = %#v", book.Authors)
	}
	if book.Pages != 328 {
		t.Fatalf("pages = %d", book.Pages)
	}
	if book.Description != "Humanity's first colony faces an unexpected threat." {
		t.Fatalf("description = %q", book.Description)
	}
	if book.Cover != "https://example.com/cover.jpg" {
		t.Fatalf("cover = %q", book.Cover)
	}
	if book.URL != "https://www.goodreads.com/book/show/36284236-genesis" {
		t.Fatalf("url = %q", book.URL)
	}
	if book.Year != "2017" {
		t.Fatalf("year = %q", book.Year)
	}
}

func TestExtractBooksFromSearchHits(t *testing.T) {
	data := map[string]interface{}{
		"data": map[string]interface{}{
			"search": map[string]interface{}{
				"results": map[string]interface{}{
					"hits": []interface{}{
						map[string]interface{}{
							"document": map[string]interface{}{
								"title":        "Network Effect",
								"author_names": []interface{}{"Martha Wells"},
								"page_count":   "352",
								"slug":         "network-effect",
							},
						},
					},
				},
			},
		},
	}

	books := extractBooks(data, 5)
	if len(books) != 1 {
		t.Fatalf("expected 1 book, got %d", len(books))
	}
	book := books[0]
	if book.Pages != 352 {
		t.Fatalf("pages = %d", book.Pages)
	}
	if book.URL != "https://hardcover.app/books/network-effect" {
		t.Fatalf("url = %q", book.URL)
	}
}

func TestExtractBooksIncludesFeaturedSeries(t *testing.T) {
	data := map[string]interface{}{
		"data": map[string]interface{}{
			"search": map[string]interface{}{
				"results": []interface{}{
					map[string]interface{}{
						"title": "The Fifth Season",
						"featured_series": map[string]interface{}{
							"position": float64(1),
							"series": map[string]interface{}{
								"name": "The Broken Earth",
							},
						},
					},
				},
			},
		},
	}

	books := extractBooks(data, 5)
	if len(books) != 1 {
		t.Fatalf("expected 1 book, got %d", len(books))
	}
	if books[0].Series != "The Broken Earth, #1" {
		t.Fatalf("series = %q", books[0].Series)
	}
}
