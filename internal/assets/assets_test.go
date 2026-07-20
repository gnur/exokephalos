package assets

import (
	"bytes"
	"os"
	"path/filepath"
	"testing"
)

var tinyPNG = []byte{0x89, 'P', 'N', 'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0}

func TestImportAndCollision(t *testing.T) {
	dir := t.TempDir()
	first, err := Import(dir, "my photo.png", bytes.NewReader(tinyPNG))
	if err != nil {
		t.Fatal(err)
	}
	if first.Path != "assets/my-photo.png" || first.MIME != "image/png" || first.Hash == "" {
		t.Fatalf("asset = %+v", first)
	}
	secondData := append(append([]byte{}, tinyPNG...), 1)
	second, err := Import(dir, "my photo.png", bytes.NewReader(secondData))
	if err != nil {
		t.Fatal(err)
	}
	if second.Path == first.Path {
		t.Fatalf("collision path was not disambiguated: %q", second.Path)
	}
	if _, err := os.Stat(filepath.Join(dir, filepath.FromSlash(second.Path))); err != nil {
		t.Fatal(err)
	}
}

func TestPathRejectsTraversal(t *testing.T) {
	if _, err := Path(t.TempDir(), "assets/../note.md"); err == nil {
		t.Fatal("expected traversal rejection")
	}
	if _, err := Path(t.TempDir(), "/assets/photo.png"); err == nil {
		t.Fatal("expected absolute path rejection")
	}
}
