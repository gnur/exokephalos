package urlimport

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"path/filepath"
	"strings"
	"time"

	readability "codeberg.org/readeck/go-readability/v2"
	md "github.com/JohannesKaufmann/html-to-markdown"
	"github.com/JohannesKaufmann/html-to-markdown/plugin"
	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
	"github.com/gnur/exokephalos/internal/repo"
	"github.com/microcosm-cc/bluemonday"
)

const (
	defaultTimeout       = 15 * time.Second
	defaultMaxBodyBytes  = 8 * 1024 * 1024
	defaultMaxRedirects  = 5
	defaultRequestAgent  = "exokephalos-urlimport/1.0"
	defaultMarkdownEmpty = "(No readable content extracted.)"
)

// Result describes the note created from an imported URL.
type Result struct {
	ID          string
	Path        string
	Frontmatter map[string]interface{}
	Body        string
}

type options struct {
	allowPrivateHosts bool
}

// Option customizes import behavior.
type Option func(*options)

// WithPrivateHosts allows local/private targets. It is intended for tests.
func WithPrivateHosts() Option {
	return func(o *options) {
		o.allowPrivateHosts = true
	}
}

// Import fetches a URL, extracts readable HTML, converts it to markdown, and
// creates a type: note item in the repository.
func Import(ctx context.Context, r *repo.Repo, baseDir, rawURL string, opts ...Option) (Result, error) {
	cfg := options{}
	for _, opt := range opts {
		opt(&cfg)
	}

	pageURL, err := validateURL(rawURL)
	if err != nil {
		return Result{}, err
	}

	body, contentType, err := fetch(ctx, pageURL, cfg)
	if err != nil {
		return Result{}, err
	}
	if !isHTMLContentType(contentType) {
		return Result{}, fmt.Errorf("unsupported content type %q", contentType)
	}

	article, err := readability.FromReader(bytes.NewReader(body), pageURL)
	if err != nil {
		return Result{}, fmt.Errorf("extracting readable content: %w", err)
	}

	note, err := articleToNote(article, pageURL)
	if err != nil {
		return Result{}, err
	}

	itemID := id.GenerateID()
	now := time.Now()
	fm := map[string]interface{}{
		"type":    "note",
		"tags":    []string{},
		"id":      itemID,
		"created": now.Format("2006-01-02"),
		"title":   note.Title,
		"url":     pageURL.String(),
		"source":  "url",
	}
	if note.SiteName != "" {
		fm["site"] = note.SiteName
	}
	if note.Author != "" {
		fm["author"] = note.Author
	}
	if note.Published != "" {
		fm["published"] = note.Published
	}
	if note.Image != "" {
		fm["image"] = note.Image
	}
	if note.Excerpt != "" {
		fm["excerpt"] = note.Excerpt
	}

	path := notePath(baseDir, itemID, note.Title)
	if err := r.CreateItem(path, fm, note.Body); err != nil {
		return Result{}, err
	}

	return Result{ID: itemID, Path: path, Frontmatter: fm, Body: note.Body}, nil
}

type note struct {
	Title     string
	Body      string
	Author    string
	SiteName  string
	Published string
	Image     string
	Excerpt   string
}

func articleToNote(article readability.Article, pageURL *url.URL) (note, error) {
	title := strings.TrimSpace(article.Title())
	if title == "" {
		title = strings.TrimSpace(pageURL.Hostname())
	}
	if title == "" {
		title = "Untitled"
	}

	var htmlBuf bytes.Buffer
	if err := article.RenderHTML(&htmlBuf); err != nil {
		return note{}, fmt.Errorf("rendering article HTML: %w", err)
	}
	html := bluemonday.UGCPolicy().Sanitize(htmlBuf.String())

	converter := md.NewConverter(pageURL.Scheme+"://"+pageURL.Host, true, nil)
	converter.Use(plugin.GitHubFlavored())
	converter.Remove("script", "style", "noscript")
	body, err := converter.ConvertString(html)
	if err != nil {
		return note{}, fmt.Errorf("converting HTML to markdown: %w", err)
	}
	body = strings.TrimSpace(body)

	if body == "" {
		var textBuf bytes.Buffer
		_ = article.RenderText(&textBuf)
		body = strings.TrimSpace(textBuf.String())
	}
	if body == "" {
		body = strings.TrimSpace(article.Excerpt())
	}
	if body == "" {
		body = defaultMarkdownEmpty
	}

	published := ""
	if t, err := article.PublishedTime(); err == nil && !t.IsZero() {
		published = t.Format("2006-01-02")
	}

	fullBody := "# " + title + "\n\n" + body + "\n"
	return note{
		Title:     title,
		Body:      fullBody,
		Author:    strings.TrimSpace(article.Byline()),
		SiteName:  strings.TrimSpace(article.SiteName()),
		Published: published,
		Image:     strings.TrimSpace(article.ImageURL()),
		Excerpt:   strings.TrimSpace(article.Excerpt()),
	}, nil
}

func fetch(ctx context.Context, pageURL *url.URL, cfg options) ([]byte, string, error) {
	transport := &http.Transport{
		Proxy: http.ProxyFromEnvironment,
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			host, port, err := net.SplitHostPort(address)
			if err != nil {
				return nil, err
			}
			if !cfg.allowPrivateHosts {
				if err := rejectPrivateHost(ctx, host); err != nil {
					return nil, err
				}
			}
			var d net.Dialer
			return d.DialContext(ctx, network, net.JoinHostPort(host, port))
		},
	}
	client := &http.Client{
		Timeout:   defaultTimeout,
		Transport: transport,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			if len(via) >= defaultMaxRedirects {
				return errors.New("too many redirects")
			}
			if req.URL.Scheme != "http" && req.URL.Scheme != "https" {
				return errors.New("redirected to unsupported URL scheme")
			}
			if !cfg.allowPrivateHosts {
				return rejectPrivateHost(req.Context(), req.URL.Hostname())
			}
			return nil
		},
	}

	req, err := http.NewRequestWithContext(ctx, http.MethodGet, pageURL.String(), nil)
	if err != nil {
		return nil, "", err
	}
	req.Header.Set("User-Agent", defaultRequestAgent)
	req.Header.Set("Accept", "text/html,application/xhtml+xml")

	resp, err := client.Do(req)
	if err != nil {
		return nil, "", err
	}
	defer resp.Body.Close()

	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return nil, "", fmt.Errorf("fetch returned status %d", resp.StatusCode)
	}

	limited := io.LimitReader(resp.Body, defaultMaxBodyBytes+1)
	body, err := io.ReadAll(limited)
	if err != nil {
		return nil, "", err
	}
	if len(body) > defaultMaxBodyBytes {
		return nil, "", fmt.Errorf("response body exceeds %d bytes", defaultMaxBodyBytes)
	}
	return body, resp.Header.Get("Content-Type"), nil
}

func validateURL(rawURL string) (*url.URL, error) {
	rawURL = strings.TrimSpace(rawURL)
	if rawURL == "" {
		return nil, errors.New("missing URL")
	}
	pageURL, err := url.Parse(rawURL)
	if err != nil {
		return nil, err
	}
	if pageURL.Scheme != "http" && pageURL.Scheme != "https" {
		return nil, errors.New("URL must use http or https")
	}
	if pageURL.Hostname() == "" {
		return nil, errors.New("URL must include a host")
	}
	return pageURL, nil
}

func rejectPrivateHost(ctx context.Context, host string) error {
	ips, err := net.DefaultResolver.LookupIPAddr(ctx, host)
	if err != nil {
		return err
	}
	if len(ips) == 0 {
		return fmt.Errorf("host %q did not resolve", host)
	}
	for _, addr := range ips {
		if isPrivateIP(addr.IP) {
			return fmt.Errorf("host %q resolves to private address", host)
		}
	}
	return nil
}

func isPrivateIP(ip net.IP) bool {
	if ip == nil {
		return true
	}
	if ip.IsLoopback() || ip.IsLinkLocalMulticast() || ip.IsLinkLocalUnicast() || ip.IsPrivate() || ip.IsUnspecified() {
		return true
	}
	return false
}

func isHTMLContentType(contentType string) bool {
	contentType = strings.ToLower(contentType)
	return contentType == "" || strings.Contains(contentType, "text/html") || strings.Contains(contentType, "application/xhtml+xml")
}

func notePath(baseDir, itemID, title string) string {
	fileName := itemID + ".md"
	if slug := markdown.Slugify(title); slug != "" {
		fileName = itemID + "-" + slug + ".md"
	}
	return filepath.Join(baseDir, itemID[:3], fileName)
}
