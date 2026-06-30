package lsp

import (
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
