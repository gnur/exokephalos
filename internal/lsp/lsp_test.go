package lsp

import (
	"context"
	"testing"
)

func TestParseWikilinks(t *testing.T) {
	tests := []struct {
		name     string
		text     string
		expected []Wikilink
	}{
		{
			name:     "no links",
			text:     "Hello world",
			expected: nil,
		},
		{
			name: "single link",
			text: "See [[abc123]] for details",
			expected: []Wikilink{
				{ID: "abc123", Start: 4, End: 14, Line: 0, StartCol: 4, EndCol: 14},
			},
		},
		{
			name: "multiple links",
			text: "Link [[foo]] and [[bar]]",
			expected: []Wikilink{
				{ID: "foo", Start: 5, End: 12, Line: 0, StartCol: 5, EndCol: 12},
				{ID: "bar", Start: 17, End: 24, Line: 0, StartCol: 17, EndCol: 24},
			},
		},
		{
			name: "link on second line",
			text: "First line\nSecond [[link]] here",
			expected: []Wikilink{
				{ID: "link", Start: 17, End: 25, Line: 1, StartCol: 7, EndCol: 15},
			},
		},
		{
			name:     "incomplete link",
			text:     "See [[abc for details",
			expected: nil,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := ParseWikilinks(tt.text)
			if len(result) != len(tt.expected) {
				t.Errorf("expected %d links, got %d", len(tt.expected), len(result))
				return
			}
			for i, exp := range tt.expected {
				if result[i].ID != exp.ID {
					t.Errorf("link %d: expected ID %q, got %q", i, exp.ID, result[i].ID)
				}
				if result[i].Line != exp.Line {
					t.Errorf("link %d: expected Line %d, got %d", i, exp.Line, result[i].Line)
				}
			}
		})
	}
}

func TestWikilinkAtPosition(t *testing.T) {
	text := "See [[abc123]] for details\nAnd [[xyz]] here"

	tests := []struct {
		name     string
		line     int
		char     int
		expected string
	}{
		{"inside first link", 0, 7, "abc123"},
		{"at start of first link", 0, 4, "abc123"},
		{"at end of first link", 0, 13, "abc123"},
		{"outside links", 0, 3, ""},
		{"inside second link", 1, 7, "xyz"},
		{"between lines", 0, 25, ""},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			link := WikilinkAtPosition(text, tt.line, tt.char)
			if tt.expected == "" {
				if link != nil {
					t.Errorf("expected no link, got %q", link.ID)
				}
			} else {
				if link == nil {
					t.Errorf("expected link %q, got nil", tt.expected)
				} else if link.ID != tt.expected {
					t.Errorf("expected link %q, got %q", tt.expected, link.ID)
				}
			}
		})
	}
}

func TestIsInsideWikilink(t *testing.T) {
	tests := []struct {
		name     string
		line     string
		char     int
		expected bool
	}{
		{"inside link", "See [[abc]] here", 7, true},
		{"before link", "See [[abc]] here", 3, false},
		{"after link", "See [[abc]] here", 14, false},
		{"at open bracket", "See [[abc]] here", 4, false},
		{"partial link", "See [[ab here", 9, true},
		{"closed then open", "See [[a]] then [[b", 15, false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := IsInsideWikilink(tt.line, tt.char)
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

func TestDetectCompletionContext(t *testing.T) {
	tests := []struct {
		name        string
		text        string
		line        int
		char        int
		expectedCtx CompletionContext
		expectedPfx string
	}{
		{
			name:        "wikilink completion",
			text:        "See [[ab",
			line:        0,
			char:        8,
			expectedCtx: CompletionContextWikilink,
			expectedPfx: "ab",
		},
		{
			name:        "frontmatter tags inline",
			text:        "---\ntags: [foo, b\n---",
			line:        1,
			char:        13,
			expectedCtx: CompletionContextFrontmatterTags,
			expectedPfx: "b",
		},
		{
			name:        "body tag",
			text:        "Some text :ta",
			line:        0,
			char:        13,
			expectedCtx: CompletionContextBodyTag,
			expectedPfx: "ta",
		},
		{
			name:        "no completion",
			text:        "Just regular text",
			line:        0,
			char:        10,
			expectedCtx: CompletionContextNone,
			expectedPfx: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx, pfx := DetectCompletionContext(tt.text, tt.line, tt.char)
			if ctx != tt.expectedCtx {
				t.Errorf("expected context %v, got %v", tt.expectedCtx, ctx)
			}
			if pfx != tt.expectedPfx {
				t.Errorf("expected prefix %q, got %q", tt.expectedPfx, pfx)
			}
		})
	}
}

func TestIsListItem(t *testing.T) {
	tests := []struct {
		name     string
		line     string
		expected bool
	}{
		{"dash list", "- item", true},
		{"asterisk list", "* item", true},
		{"numbered list", "1. item", true},
		{"numbered list two digits", "12. item", true},
		{"indented dash", "  - item", true},
		{"not a list", "regular text", false},
		{"empty", "", false},
		{"just dash", "-", false},
		{"dash no space", "-item", false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := isListItem(tt.line)
			if result != tt.expected {
				t.Errorf("expected %v, got %v", tt.expected, result)
			}
		})
	}
}

func TestGetCodeActions(t *testing.T) {
	tests := []struct {
		name           string
		text           string
		line           int
		expectActions  int
	}{
		{
			name:          "dash list item",
			text:          "- Buy groceries",
			line:          0,
			expectActions: 1,
		},
		{
			name:          "asterisk list item",
			text:          "* Buy groceries",
			line:          0,
			expectActions: 1,
		},
		{
			name:          "numbered list item",
			text:          "1. Buy groceries",
			line:          0,
			expectActions: 1,
		},
		{
			name:          "regular text",
			text:          "Buy groceries",
			line:          0,
			expectActions: 0,
		},
		{
			name:          "indented list item",
			text:          "  - Buy groceries",
			line:          0,
			expectActions: 1,
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			actions, err := GetCodeActions(context.Background(), tt.text, tt.line, "file:///test.md")
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if len(actions) != tt.expectActions {
				t.Errorf("expected %d actions, got %d", tt.expectActions, len(actions))
			}
		})
	}
}

func TestGetCodeActions_TodoDone(t *testing.T) {
	tests := []struct {
		name          string
		text          string
		expectActions int
		expectTitles  []string
	}{
		{
			name: "note with todo tag",
			text: `---
type: note
tags: [todo]
---

# Test note`,
			expectActions: 1,
			expectTitles:  []string{"Mark as done"},
		},
		{
			name: "note with todo and done tags",
			text: `---
type: note
tags: [todo, done]
---

# Test note`,
			expectActions: 0,
			expectTitles:  []string{},
		},
		{
			name: "note without todo tag",
			text: `---
type: note
tags: [other]
---

# Test note`,
			expectActions: 0,
			expectTitles:  []string{},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			actions, err := GetCodeActions(context.Background(), tt.text, 0, "file:///test.md")
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if len(actions) != tt.expectActions {
				t.Errorf("expected %d actions, got %d", tt.expectActions, len(actions))
			}
			for i, title := range tt.expectTitles {
				if i >= len(actions) {
					t.Errorf("missing action with title %q", title)
					continue
				}
				if actions[i].Title != title {
					t.Errorf("expected title %q, got %q", title, actions[i].Title)
				}
			}
		})
	}
}

func TestGetCodeActions_BookStatus(t *testing.T) {
	tests := []struct {
		name          string
		text          string
		expectActions int
		expectTitles  []string
	}{
		{
			name: "book with to-read tag",
			text: `---
type: book
tags: [to-read]
---

# Test book`,
			expectActions: 2,
			expectTitles:  []string{"Mark as reading", "Mark as read"},
		},
		{
			name: "book with reading tag",
			text: `---
type: book
tags: [reading]
---

# Test book`,
			expectActions: 2,
			expectTitles:  []string{"Mark as to-read", "Mark as read"},
		},
		{
			name: "book with read tag",
			text: `---
type: book
tags: [read]
---

# Test book`,
			expectActions: 2,
			expectTitles:  []string{"Mark as to-read", "Mark as reading"},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			actions, err := GetCodeActions(context.Background(), tt.text, 0, "file:///test.md")
			if err != nil {
				t.Fatalf("unexpected error: %v", err)
			}
			if len(actions) != tt.expectActions {
				t.Errorf("expected %d actions, got %d", tt.expectActions, len(actions))
			}
			for i, title := range tt.expectTitles {
				if i >= len(actions) {
					t.Errorf("missing action with title %q", title)
					continue
				}
				if actions[i].Title != title {
					t.Errorf("expected title %q, got %q", title, actions[i].Title)
				}
			}
		})
	}
}

func TestAddTagToLine(t *testing.T) {
	tests := []struct {
		name     string
		line     string
		tag      string
		expected string
	}{
		{
			name:     "add to empty array",
			line:     "tags: []",
			tag:      "reading",
			expected: "tags: [reading]",
		},
		{
			name:     "add to existing array",
			line:     "tags: [to-read]",
			tag:      "done",
			expected: "tags: [to-read, done]",
		},
		{
			name:     "add to array with multiple tags",
			line:     "tags: [to-read, fiction]",
			tag:      "favorite",
			expected: "tags: [to-read, fiction, favorite]",
		},
		{
			name:     "tag already exists",
			line:     "tags: [reading]",
			tag:      "reading",
			expected: "tags: [reading]",
		},
		{
			name:     "add to yaml list",
			line:     "tags:",
			tag:      "reading",
			expected: "tags:\n  - reading",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := addTagToLine(tt.line, tt.tag)
			if result != tt.expected {
				t.Errorf("expected %q, got %q", tt.expected, result)
			}
		})
	}
}

func TestRemoveTagFromLine(t *testing.T) {
	tests := []struct {
		name     string
		line     string
		tag      string
		expected string
	}{
		{
			name:     "remove only tag",
			line:     "tags: [reading]",
			tag:      "reading",
			expected: "tags: []",
		},
		{
			name:     "remove first tag",
			line:     "tags: [reading, fiction]",
			tag:      "reading",
			expected: "tags: [fiction]",
		},
		{
			name:     "remove last tag",
			line:     "tags: [fiction, reading]",
			tag:      "reading",
			expected: "tags: [fiction]",
		},
		{
			name:     "remove middle tag",
			line:     "tags: [fiction, reading, favorite]",
			tag:      "reading",
			expected: "tags: [fiction, favorite]",
		},
		{
			name:     "tag not present",
			line:     "tags: [fiction]",
			tag:      "reading",
			expected: "tags: [fiction]",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			result := removeTagFromLine(tt.line, tt.tag)
			if result != tt.expected {
				t.Errorf("expected %q, got %q", tt.expected, result)
			}
		})
	}
}

func TestGetSemanticTokens(t *testing.T) {
	tests := []struct {
		name     string
		text     string
		checkFn  func([]uint32) bool
	}{
		{
			name: "wikilink decorator token",
			text: "See [[abc123]] here",
			checkFn: func(tokens []uint32) bool {
				if len(tokens) < 5 {
					return false
				}
				deltaLine := tokens[0]
				deltaChar := tokens[1]
				length := tokens[2]
				tokenType := tokens[3]
				return deltaLine == 0 && deltaChar == 4 && length == 10 && tokenType == 0
			},
		},
		{
			name: "body tag keyword token",
			text: "---\ntags: []\n---\n\nSee :mytag: here",
			checkFn: func(tokens []uint32) bool {
				if len(tokens) < 5 {
					return false
				}
				tokenType := tokens[3]
				return tokenType == 1
			},
		},
		{
			name: "no frontmatter tags in body",
			text: "---\ntags: [test]\n---\n\nNo tags here",
			checkFn: func(tokens []uint32) bool {
				return len(tokens) == 0
			},
		},
		{
			name: "multiple wikilinks",
			text: "[[a]] and [[b]]",
			checkFn: func(tokens []uint32) bool {
				return len(tokens) >= 10
			},
		},
		{
			name: "empty document",
			text: "",
			checkFn: func(tokens []uint32) bool {
				return len(tokens) == 0
			},
		},
		{
			name: "hash tag keyword token",
			text: "---\ntags: []\n---\n\nSee #mytag here",
			checkFn: func(tokens []uint32) bool {
				if len(tokens) < 5 {
					return false
				}
				// First token should be hash tag at line 4 (delta=4), char 4, length 6 ("#mytag")
				deltaLine := tokens[0]
				deltaChar := tokens[1]
				length := tokens[2]
				tokenType := tokens[3]
				return deltaLine == 4 && deltaChar == 4 && length == 6 && tokenType == 1
			},
		},
		{
			name: "mixed colon and hash tags",
			text: "---\ntags: []\n---\n\n:colon: and #hash",
			checkFn: func(tokens []uint32) bool {
				// Should have at least 2 tokens (both tags)
				return len(tokens) >= 10
			},
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			tokens := GetSemanticTokens(tt.text)
			if !tt.checkFn(tokens) {
				t.Errorf("token validation failed for %q, got %v", tt.name, tokens)
			}
		})
	}
}

func TestDetectHashTagContext(t *testing.T) {
	tests := []struct {
		name        string
		prefix      string
		expectedCtx CompletionContext
		expectedPfx string
	}{
		{
			name:        "hash tag at start",
			prefix:      "#my",
			expectedCtx: CompletionContextBodyTag,
			expectedPfx: "my",
		},
		{
			name:        "hash tag with space before",
			prefix:      "text #my",
			expectedCtx: CompletionContextBodyTag,
			expectedPfx: "my",
		},
		{
			name:        "hash in word (not tag)",
			prefix:      "ab#cd",
			expectedCtx: CompletionContextNone,
			expectedPfx: "",
		},
		{
			name:        "hash in heading (not tag)",
			prefix:      "# Heading",
			expectedCtx: CompletionContextNone,
			expectedPfx: "",
		},
		{
			name:        "hash after punctuation",
			prefix:      "text(#tag",
			expectedCtx: CompletionContextBodyTag,
			expectedPfx: "tag",
		},
		{
			name:        "no hash",
			prefix:      "regular text",
			expectedCtx: CompletionContextNone,
			expectedPfx: "",
		},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			ctx, pfx := detectHashTagContext(tt.prefix)
			if ctx != tt.expectedCtx {
				t.Errorf("expected context %v, got %v", tt.expectedCtx, ctx)
			}
			if pfx != tt.expectedPfx {
				t.Errorf("expected prefix %q, got %q", tt.expectedPfx, pfx)
			}
		})
	}
}
