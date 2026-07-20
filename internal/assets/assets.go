// Package assets manages image files stored alongside a workspace.
package assets

import (
	"crypto/sha256"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
)

const MaxImageSize = 20 << 20

var allowed = map[string]string{
	"image/jpeg": ".jpg", "image/png": ".png", "image/gif": ".gif", "image/webp": ".webp",
}

type Asset struct {
	Path, MIME, Hash string
	Size             int64
}

func Import(baseDir, originalName string, r io.Reader) (Asset, error) {
	data, err := io.ReadAll(io.LimitReader(r, MaxImageSize+1))
	if err != nil {
		return Asset{}, err
	}
	if len(data) > MaxImageSize {
		return Asset{}, fmt.Errorf("image exceeds 20 MiB limit")
	}
	mime := http.DetectContentType(data)
	ext, ok := allowed[mime]
	if !ok {
		return Asset{}, fmt.Errorf("unsupported image type %q", mime)
	}
	name := sanitize(originalName)
	if filepath.Ext(name) == "" {
		name += ext
	}
	dir := filepath.Join(baseDir, "assets")
	if err := os.MkdirAll(dir, 0755); err != nil {
		return Asset{}, err
	}
	hash := fmt.Sprintf("%x", sha256.Sum256(data))
	path := filepath.Join(dir, name)
	if existing, err := os.ReadFile(path); err == nil && string(existing) != string(data) {
		base, suffix := strings.TrimSuffix(name, filepath.Ext(name)), filepath.Ext(name)
		path = filepath.Join(dir, base+"-"+hash[:8]+suffix)
	}
	if _, err := os.Stat(path); os.IsNotExist(err) {
		if err := os.WriteFile(path, data, 0644); err != nil {
			return Asset{}, err
		}
	}
	rel, _ := filepath.Rel(baseDir, path)
	return Asset{Path: filepath.ToSlash(rel), MIME: mime, Hash: hash, Size: int64(len(data))}, nil
}

// Inspect returns metadata for an existing workspace asset. It deliberately
// only accepts the same image formats as Import so that sync never publishes
// arbitrary files from the assets directory.
func Inspect(baseDir, rel string) (Asset, error) {
	path, err := Path(baseDir, rel)
	if err != nil {
		return Asset{}, err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return Asset{}, err
	}
	if len(data) > MaxImageSize {
		return Asset{}, fmt.Errorf("image exceeds 20 MiB limit")
	}
	mime := http.DetectContentType(data)
	if _, ok := allowed[mime]; !ok {
		return Asset{}, fmt.Errorf("unsupported image type %q", mime)
	}
	hash := fmt.Sprintf("%x", sha256.Sum256(data))
	return Asset{Path: filepath.ToSlash(rel), MIME: mime, Hash: hash, Size: int64(len(data))}, nil
}

// Store writes a validated image to its exact workspace-relative path. Sync
// uses this rather than Import because the path is part of the signed metadata.
func Store(baseDir, rel string, r io.Reader) (Asset, error) {
	path, err := Path(baseDir, rel)
	if err != nil {
		return Asset{}, err
	}
	data, err := io.ReadAll(io.LimitReader(r, MaxImageSize+1))
	if err != nil {
		return Asset{}, err
	}
	if len(data) > MaxImageSize {
		return Asset{}, fmt.Errorf("image exceeds 20 MiB limit")
	}
	mime := http.DetectContentType(data)
	if _, ok := allowed[mime]; !ok {
		return Asset{}, fmt.Errorf("unsupported image type %q", mime)
	}
	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		return Asset{}, err
	}
	if err := os.WriteFile(path, data, 0644); err != nil {
		return Asset{}, err
	}
	hash := fmt.Sprintf("%x", sha256.Sum256(data))
	return Asset{Path: filepath.ToSlash(rel), MIME: mime, Hash: hash, Size: int64(len(data))}, nil
}

func Path(baseDir, rel string) (string, error) {
	clean := filepath.Clean(filepath.FromSlash(rel))
	if filepath.IsAbs(clean) || clean == "assets" || !strings.HasPrefix(clean, "assets"+string(filepath.Separator)) {
		return "", fmt.Errorf("invalid asset path")
	}
	path := filepath.Join(baseDir, clean)
	if !strings.HasPrefix(path, filepath.Join(baseDir, "assets")+string(filepath.Separator)) {
		return "", fmt.Errorf("invalid asset path")
	}
	return path, nil
}

func sanitize(name string) string {
	name = filepath.Base(name)
	name = strings.Map(func(r rune) rune {
		if r == '-' || r == '_' || r == '.' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' {
			return r
		}
		return '-'
	}, name)
	name = strings.Trim(name, ".-")
	if name == "" {
		return "image"
	}
	return name
}
