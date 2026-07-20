package handlers

import (
	"net/http"
	"path/filepath"

	"github.com/gnur/exokephalos/internal/assets"
	"github.com/gnur/exokephalos/internal/syncsvc"
)

func (h *Handlers) AppAssetUpload(w http.ResponseWriter, r *http.Request) {
	if err := r.ParseMultipartForm(assets.MaxImageSize + 1024); err != nil {
		writeAPIError(w, "invalid upload", http.StatusBadRequest)
		return
	}
	file, header, err := r.FormFile("image")
	if err != nil {
		writeAPIError(w, "image is required", http.StatusBadRequest)
		return
	}
	defer file.Close()
	asset, err := assets.Import(h.BaseDir, header.Filename, file)
	if err != nil {
		writeAPIError(w, err.Error(), http.StatusBadRequest)
		return
	}
	if h.SyncServer != nil {
		if _, err := h.SyncServer.ApplyChange(syncsvc.Change{Op: "upsert_asset", TargetKind: "asset", Path: asset.Path, Hash: asset.Hash, MIME: asset.MIME, Size: asset.Size}); err != nil {
			writeAPIError(w, "recording asset metadata", http.StatusInternalServerError)
			return
		}
	}
	writeAppJSON(w, map[string]string{"path": asset.Path, "markdown": "![" + filepath.Base(asset.Path) + "](" + asset.Path + ")"})
}

func (h *Handlers) Asset(w http.ResponseWriter, r *http.Request) {
	path, err := assets.Path(h.BaseDir, "assets/"+r.PathValue("path"))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	asset, err := assets.Inspect(h.BaseDir, "assets/"+r.PathValue("path"))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", asset.MIME)
	w.Header().Set("X-Content-Type-Options", "nosniff")
	http.ServeFile(w, r, path)
}
