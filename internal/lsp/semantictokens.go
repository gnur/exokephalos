package lsp

import (
	"regexp"
	"strings"

	"github.com/modern-dev/go-lsp/protocol"
)

var SemanticTokenLegend = protocol.SemanticTokensLegend{
	TokenTypes:    []string{"decorator", "keyword"},
	TokenModifiers: []string{},
}

var bodyTagTokenRegex = regexp.MustCompile(`:[a-z0-9_-]+:|#[a-z0-9_-]+`)

const (
	semTokenTypeDecorator = 0
	semTokenTypeKeyword   = 1
)

func GetSemanticTokens(text string) []uint32 {
	var data []uint32
	var prevLine, prevChar uint32

	fmEnd := findFrontmatterEnd(text)

	lines := strings.Split(text, "\n")
	for lineIdx, line := range lines {
		links := ParseWikilinks(line)
		for _, link := range links {
			deltaLine := uint32(lineIdx) - prevLine
			deltaChar := uint32(link.StartCol)
			if deltaLine == 0 {
				deltaChar = uint32(link.StartCol) - prevChar
			}
			length := uint32(link.EndCol - link.StartCol)

			data = append(data, deltaLine, deltaChar, length, semTokenTypeDecorator, 0)
			prevLine = uint32(lineIdx)
			prevChar = uint32(link.StartCol)
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

			deltaLine := uint32(lineIdx) - prevLine
			deltaChar := uint32(m[0])
			if deltaLine == 0 {
				deltaChar = uint32(m[0]) - prevChar
			}
			length := uint32(m[1] - m[0])

			data = append(data, deltaLine, deltaChar, length, semTokenTypeKeyword, 0)
			prevLine = uint32(lineIdx)
			prevChar = uint32(m[0])
		}
	}

	return data
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
