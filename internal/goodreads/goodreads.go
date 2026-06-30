package goodreads

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"

	"golang.org/x/net/html"
)

// BookMetadata holds the extracted book information from a Goodreads page.
type BookMetadata struct {
	Title  string
	Author []string
	Pages  int
	Cover  string
	URL    string
}

// ldJSON represents the JSON-LD structured data on a Goodreads book page.
type ldJSON struct {
	Type          string     `json:"@type"`
	Name          string     `json:"name"`
	Image         string     `json:"image"`
	NumberOfPages int        `json:"numberOfPages"`
	Authors       []ldAuthor `json:"author"`
}

type ldAuthor struct {
	Name string `json:"name"`
}

// FetchBook fetches a Goodreads book URL and extracts metadata from the JSON-LD script tag.
func FetchBook(rawURL string) (*BookMetadata, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, fmt.Errorf("parsing URL: %w", err)
	}
	if u.Scheme != "http" && u.Scheme != "https" {
		return nil, fmt.Errorf("invalid URL scheme %q", u.Scheme)
	}
	if u.Host != "goodreads.com" && u.Host != "www.goodreads.com" {
		return nil, fmt.Errorf("invalid URL host %q, must be goodreads.com", u.Host)
	}

	// Clean URL (remove query params)
	cleanURL := rawURL
	if idx := strings.Index(rawURL, "?"); idx != -1 {
		cleanURL = rawURL[:idx]
	}

	req, err := http.NewRequest("GET", rawURL, nil)
	if err != nil {
		return nil, fmt.Errorf("creating request: %w", err)
	}
	req.Header.Set("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
	req.Header.Set("Accept", "text/html,application/xhtml+xml")

	client := &http.Client{
		Timeout: 10 * time.Second,
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("fetching page: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading body: %w", err)
	}

	// Parse HTML and find JSON-LD script tag
	doc, err := html.Parse(strings.NewReader(string(body)))
	if err != nil {
		return nil, fmt.Errorf("parsing HTML: %w", err)
	}

	jsonLD := findJSONLD(doc)
	if jsonLD == "" {
		return nil, fmt.Errorf("no JSON-LD data found on page")
	}

	var data ldJSON
	if err := json.Unmarshal([]byte(jsonLD), &data); err != nil {
		return nil, fmt.Errorf("parsing JSON-LD: %w", err)
	}

	if data.Type != "Book" {
		return nil, fmt.Errorf("JSON-LD type is %q, expected Book", data.Type)
	}

	var authors []string
	for _, a := range data.Authors {
		if a.Name != "" {
			authors = append(authors, a.Name)
		}
	}

	return &BookMetadata{
		Title:  data.Name,
		Author: authors,
		Pages:  data.NumberOfPages,
		Cover:  data.Image,
		URL:    cleanURL,
	}, nil
}

// findJSONLD traverses the HTML tree looking for a <script type="application/ld+json"> tag.
func findJSONLD(n *html.Node) string {
	if n.Type == html.ElementNode && n.Data == "script" {
		for _, attr := range n.Attr {
			if attr.Key == "type" && attr.Val == "application/ld+json" {
				// Get text content
				if n.FirstChild != nil && n.FirstChild.Type == html.TextNode {
					return n.FirstChild.Data
				}
			}
		}
	}
	for c := n.FirstChild; c != nil; c = c.NextSibling {
		if result := findJSONLD(c); result != "" {
			return result
		}
	}
	return ""
}
