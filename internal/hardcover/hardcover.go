package hardcover

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"regexp"
	"strconv"
	"strings"
	"time"
)

const (
	APIURL    = "https://api.hardcover.app/v1/graphql"
	userAgent = "exokephalos/1.0"
)

// Book is the normalized metadata used by exokephalos.
type Book struct {
	Title       string
	Authors     []string
	Description string
	Pages       int
	Cover       string
	URL         string
	Year        string
	Series      string
}

type Client struct {
	token      string
	httpClient *http.Client
}

func NewClient(token string) *Client {
	return &Client{
		token: strings.TrimSpace(token),
		httpClient: &http.Client{
			Timeout: 30 * time.Second,
		},
	}
}

func (c *Client) Search(query string, limit int) ([]Book, error) {
	query = strings.TrimSpace(query)
	if query == "" {
		return nil, fmt.Errorf("missing query")
	}
	if c.token == "" {
		return nil, fmt.Errorf("HARDCOVER_TOKEN is not set")
	}
	if limit <= 0 {
		limit = 5
	}

	graphql := `
query SearchBooks($query: String!, $perPage: Int!, $page: Int!) {
  search(query: $query, query_type: "Book", per_page: $perPage, page: $page) {
    results
  }
}`
	payload := map[string]interface{}{
		"query": graphql,
		"variables": map[string]interface{}{
			"query":   query,
			"perPage": limit,
			"page":    1,
		},
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}

	req, err := http.NewRequest(http.MethodPost, APIURL, bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", c.token)
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("User-Agent", userAgent)

	resp, err := c.httpClient.Do(req)
	if err != nil {
		return nil, fmt.Errorf("hardcover search: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, fmt.Errorf("hardcover search HTTP %d", resp.StatusCode)
	}

	var parsed map[string]interface{}
	if err := json.NewDecoder(resp.Body).Decode(&parsed); err != nil {
		return nil, fmt.Errorf("decoding hardcover response: %w", err)
	}
	if errors, ok := parsed["errors"]; ok {
		return nil, fmt.Errorf("hardcover returned errors: %v", errors)
	}

	books := extractBooks(parsed, limit)
	if len(books) == 0 {
		return nil, fmt.Errorf("hardcover returned no book results")
	}
	return books, nil
}

func extractBooks(data map[string]interface{}, limit int) []Book {
	search, _ := nestedMap(data, "data", "search")
	results := search["results"]
	raw := extractResultMaps(results, limit)

	books := make([]Book, 0, len(raw))
	for _, result := range raw {
		book := normalizeBook(result)
		if book.Title == "" {
			continue
		}
		books = append(books, book)
	}
	return books
}

func extractResultMaps(results interface{}, limit int) []map[string]interface{} {
	var raw []map[string]interface{}
	switch typed := results.(type) {
	case []interface{}:
		for _, item := range typed {
			if m, ok := item.(map[string]interface{}); ok {
				raw = append(raw, m)
			}
		}
	case map[string]interface{}:
		if hits, ok := typed["hits"].([]interface{}); ok {
			for _, hit := range hits {
				hitMap, ok := hit.(map[string]interface{})
				if !ok {
					continue
				}
				if document, ok := hitMap["document"].(map[string]interface{}); ok {
					raw = append(raw, document)
				}
			}
		}
	}
	if limit > 0 && len(raw) > limit {
		return raw[:limit]
	}
	return raw
}

func normalizeBook(result map[string]interface{}) Book {
	book := Book{
		Title:       stringValue(result, "title"),
		Authors:     stringSliceValue(result, "author_names"),
		Description: stringValue(result, "description"),
		Pages:       intValue(result, "pages", "page_count", "pageCount"),
		Cover:       urlValue(result, "image", "image_url", "cover", "cover_url"),
		URL:         bookURL(result),
		Year:        stringValue(result, "release_year"),
		Series:      seriesValue(result),
	}
	if len(book.Authors) == 0 {
		book.Authors = stringSliceValue(result, "authors")
	}
	if book.Pages == 0 {
		if metadata, ok := result["metadata"].(map[string]interface{}); ok {
			book.Pages = intValue(metadata, "pageCount", "pages")
		}
	}
	return book
}

func bookURL(result map[string]interface{}) string {
	for _, key := range []string{"goodreads_url", "url", "canonical_url"} {
		if value := stringValue(result, key); value != "" {
			return value
		}
	}
	if externalIDs, ok := result["external_ids"].(map[string]interface{}); ok {
		if id := stringValue(externalIDs, "goodreads", "goodreads_id", "goodreadsId"); id != "" {
			return "https://www.goodreads.com/book/show/" + id
		}
	}
	if id := stringValue(result, "goodreads_id", "goodreadsId"); id != "" {
		return "https://www.goodreads.com/book/show/" + id
	}
	if slug := stringValue(result, "slug"); slug != "" {
		return "https://hardcover.app/books/" + slug
	}
	if id := stringValue(result, "id"); id != "" {
		return "https://hardcover.app/books/" + id
	}
	return ""
}

func seriesValue(result map[string]interface{}) string {
	if featured, ok := result["featured_series"].(map[string]interface{}); ok {
		if series, ok := featured["series"].(map[string]interface{}); ok {
			name := stringValue(series, "name")
			position := scalarString(featured["position"])
			if name != "" && position != "" {
				return name + " #" + position
			}
			return name
		}
	}
	return firstString(result["series_names"])
}

func nestedMap(data map[string]interface{}, keys ...string) (map[string]interface{}, bool) {
	current := data
	for _, key := range keys {
		next, ok := current[key].(map[string]interface{})
		if !ok {
			return nil, false
		}
		current = next
	}
	return current, true
}

func stringValue(data map[string]interface{}, keys ...string) string {
	for _, key := range keys {
		if value := scalarString(data[key]); value != "" {
			return value
		}
	}
	return ""
}

func stringSliceValue(data map[string]interface{}, key string) []string {
	value := data[key]
	switch typed := value.(type) {
	case []interface{}:
		out := make([]string, 0, len(typed))
		for _, item := range typed {
			if s := scalarString(item); s != "" {
				out = append(out, s)
				continue
			}
			if m, ok := item.(map[string]interface{}); ok {
				if s := stringValue(m, "name", "title"); s != "" {
					out = append(out, s)
				}
			}
		}
		return out
	case []string:
		return typed
	case string:
		if typed != "" {
			return []string{typed}
		}
	}
	return nil
}

func intValue(data map[string]interface{}, keys ...string) int {
	for _, key := range keys {
		switch value := data[key].(type) {
		case int:
			return value
		case float64:
			return int(value)
		case string:
			if parsed, err := strconv.Atoi(strings.TrimSpace(value)); err == nil {
				return parsed
			}
		}
	}
	return 0
}

func urlValue(data map[string]interface{}, keys ...string) string {
	for _, key := range keys {
		if value := scalarString(data[key]); strings.HasPrefix(value, "http://") || strings.HasPrefix(value, "https://") {
			return value
		}
		if m, ok := data[key].(map[string]interface{}); ok {
			if value := stringValue(m, "url", "image_url", "cover_url"); strings.HasPrefix(value, "http://") || strings.HasPrefix(value, "https://") {
				return value
			}
		}
	}
	return ""
}

func firstString(value interface{}) string {
	switch typed := value.(type) {
	case []interface{}:
		for _, item := range typed {
			if s := scalarString(item); s != "" {
				return s
			}
		}
	case []string:
		for _, item := range typed {
			if item != "" {
				return item
			}
		}
	case string:
		return typed
	}
	return ""
}

func scalarString(value interface{}) string {
	switch typed := value.(type) {
	case string:
		return strings.TrimSpace(stripHTML(typed))
	case float64:
		if typed == float64(int64(typed)) {
			return strconv.FormatInt(int64(typed), 10)
		}
		return strconv.FormatFloat(typed, 'f', -1, 64)
	case int:
		return strconv.Itoa(typed)
	case json.Number:
		return typed.String()
	}
	return ""
}

var htmlTagRE = regexp.MustCompile(`<[^>]+>`)

func stripHTML(value string) string {
	return strings.Join(strings.Fields(htmlTagRE.ReplaceAllString(value, " ")), " ")
}
