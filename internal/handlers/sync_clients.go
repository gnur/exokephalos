package handlers

import (
	"net/http"
	"time"
)

func (h *Handlers) SyncClients(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		http.NotFound(w, r)
		return
	}
	data := newData(r)
	parseStart := time.Now()
	clients, err := h.SyncServer.Clients()
	data["_parseTime"] = time.Since(parseStart)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	data["Clients"] = clients
	h.render(w, r, "sync/clients.html", data)
}

func (h *Handlers) SyncClientApprove(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		http.NotFound(w, r)
		return
	}
	if err := h.SyncServer.ApproveClient(r.PathValue("clientId")); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.Redirect(w, r, "/admin/sync/clients", http.StatusSeeOther)
}

func (h *Handlers) SyncClientRevoke(w http.ResponseWriter, r *http.Request) {
	if h.SyncServer == nil {
		http.NotFound(w, r)
		return
	}
	if err := h.SyncServer.RevokeClient(r.PathValue("clientId")); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	http.Redirect(w, r, "/admin/sync/clients", http.StatusSeeOther)
}
