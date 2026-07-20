package lsp

import (
	"go.lsp.dev/uri"
)

func pathToURI(path string) uri.URI {
	return uri.File(path)
}
