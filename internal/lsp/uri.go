package lsp

import (
	"net/url"
	"path/filepath"
	"runtime"
	"strings"

	"github.com/modern-dev/go-lsp/protocol"
)

func pathToURI(path string) protocol.DocumentURI {
	absPath, err := filepath.Abs(path)
	if err != nil {
		absPath = path
	}

	if runtime.GOOS == "windows" {
		absPath = "/" + strings.ReplaceAll(absPath, "\\", "/")
	}

	return protocol.DocumentURI("file://" + absPath)
}

func uriToPath(uri protocol.DocumentURI) string {
	u, err := url.Parse(string(uri))
	if err != nil {
		return strings.TrimPrefix(string(uri), "file://")
	}

	path := u.Path
	if runtime.GOOS == "windows" {
		path = strings.TrimPrefix(path, "/")
		path = strings.ReplaceAll(path, "/", "\\")
	}

	return path
}
