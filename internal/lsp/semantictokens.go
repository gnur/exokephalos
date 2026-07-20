package lsp

import (
	"regexp"
	"strings"

	"go.lsp.dev/protocol"
)

var SemanticTokenLegend = protocol.SemanticTokensLegend{
	TokenTypes:     []string{"decorator", "keyword", "property", "string", "comment"},
	TokenModifiers: []string{},
}

var bodyTagTokenRegex = regexp.MustCompile(`:[a-z0-9_-]+:|#[a-z0-9_-]+`)

const (
	semTokenTypeDecorator = 0
	semTokenTypeKeyword   = 1
	semTokenTypeProperty  = 2
	semTokenTypeString    = 3
	semTokenTypeComment   = 4
)

func GetSemanticTokens(text string) []uint32 {
	var data []uint32
	var prevLine, prevChar uint32

	fmEnd := findFrontmatterEnd(text)

	lines := strings.Split(text, "\n")
	for lineIdx, line := range lines {
		trimmed := strings.TrimSpace(line)
		if lineIdx <= fmEnd {
			if colon := strings.Index(trimmed, ":"); colon > 0 {
				start := strings.Index(line, trimmed)
				data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, start, colon, semTokenTypeProperty)
				valueStart := start + colon + 1
				if valueStart < len(line) {
					data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, valueStart, len(line)-valueStart, semTokenTypeString)
				}
			}
		}
		if strings.HasPrefix(trimmed, "#") {
			start := strings.Index(line, "#")
			data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, start, len(trimmed), semTokenTypeKeyword)
		}
		if strings.HasPrefix(trimmed, "- [") || strings.HasPrefix(trimmed, "* [") {
			start := strings.Index(line, "[")
			data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, start, 3, semTokenTypeComment)
		}
		links := ParseWikilinks(line)
		for _, link := range links {
			data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, link.StartCol, link.EndCol-link.StartCol, semTokenTypeDecorator)
		}

		if lineIdx <= fmEnd {
			continue
		}

		tagMatches := bodyTagTokenRegex.FindAllStringIndex(line, -1)
		for _, m := range tagMatches {
			tag := line[m[0]:m[1]]
			if !isValidBodyTag(tag) {
				continue
			}

			data, prevLine, prevChar = appendSemanticToken(data, prevLine, prevChar, lineIdx, m[0], m[1]-m[0], semTokenTypeKeyword)
		}
	}

	return data
}

func appendSemanticToken(data []uint32, previousLine, previousChar uint32, line, start, length int, tokenType uint32) ([]uint32, uint32, uint32) {
	if length <= 0 {
		return data, previousLine, previousChar
	}
	deltaLine := uint32(line) - previousLine
	deltaChar := uint32(start)
	if deltaLine == 0 {
		deltaChar -= previousChar
	}
	return append(data, deltaLine, deltaChar, uint32(length), tokenType, 0), uint32(line), uint32(start)
}

func isValidBodyTag(tag string) bool {
	if len(tag) < 2 {
		return false
	}

	// Handle :tag: format
	if strings.HasPrefix(tag, ":") && strings.HasSuffix(tag, ":") {
		if len(tag) < 4 {
			return false
		}
		inner := tag[1 : len(tag)-1]
		if len(inner) == 0 {
			return false
		}
		for _, c := range inner {
			if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' || c == '_') {
				return false
			}
		}
		return true
	}

	// Handle #tag format
	if strings.HasPrefix(tag, "#") {
		if len(tag) < 2 {
			return false
		}
		inner := tag[1:]
		if len(inner) == 0 {
			return false
		}
		for _, c := range inner {
			if !((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' || c == '_') {
				return false
			}
		}
		return true
	}

	return false
}
