package lsp

import (
	"regexp"
	"strings"
)

type Wikilink struct {
	ID       string
	Start    int
	End      int
	Line     int
	StartCol int
	EndCol   int
}

var wikilinkRegex = regexp.MustCompile(`\[\[([^\]]*)\]\]`)

func ParseWikilinks(text string) []Wikilink {
	var links []Wikilink
	matches := wikilinkRegex.FindAllStringSubmatchIndex(text, -1)

	lines := strings.Split(text, "\n")
	lineOffsets := make([]int, len(lines))
	offset := 0
	for i, line := range lines {
		lineOffsets[i] = offset
		offset += len(line) + 1
	}

	for _, match := range matches {
		fullStart := match[0]
		fullEnd := match[1]
		idStart := match[2]
		idEnd := match[3]

		id := text[idStart:idEnd]

		line, startCol := offsetToLineCol(lineOffsets, fullStart)
		_, endCol := offsetToLineCol(lineOffsets, fullEnd)

		links = append(links, Wikilink{
			ID:       id,
			Start:    fullStart,
			End:      fullEnd,
			Line:     line,
			StartCol: startCol,
			EndCol:   endCol,
		})
	}

	return links
}

func WikilinkAtPosition(text string, line, char int) *Wikilink {
	lines := strings.Split(text, "\n")
	if line >= len(lines) {
		return nil
	}

	lineOffsets := make([]int, len(lines))
	offset := 0
	for i, l := range lines {
		lineOffsets[i] = offset
		offset += len(l) + 1
	}

	targetOffset := lineOffsets[line] + char

	links := ParseWikilinks(text)
	for i := range links {
		if targetOffset >= links[i].Start && targetOffset <= links[i].End {
			return &links[i]
		}
	}

	return nil
}

func offsetToLineCol(lineOffsets []int, offset int) (int, int) {
	for i := len(lineOffsets) - 1; i >= 0; i-- {
		if offset >= lineOffsets[i] {
			return i, offset - lineOffsets[i]
		}
	}
	return 0, offset
}

func IsInsideWikilink(line string, char int) bool {
	prefix := line[:char]
	openIdx := strings.LastIndex(prefix, "[[")
	if openIdx == -1 {
		return false
	}
	closeIdx := strings.LastIndex(prefix, "]]")
	return openIdx > closeIdx
}

func GetWikilinkPrefix(line string, char int) string {
	prefix := line[:char]
	openIdx := strings.LastIndex(prefix, "[[")
	if openIdx == -1 {
		return ""
	}
	closeIdx := strings.LastIndex(prefix, "]]")
	if openIdx < closeIdx {
		return ""
	}
	return prefix[openIdx+2:]
}
