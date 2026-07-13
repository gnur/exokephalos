package handlers

import (
	"net/http"
	"strings"
	"time"
)

func (h *Handlers) Login(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		http.NotFound(w, r)
		return
	}
	data := map[string]interface{}{
		"Next": r.URL.Query().Get("next"),
	}
	if r.Method == http.MethodPost {
		_ = r.ParseForm()
		password := r.FormValue("password")
		ok, err := h.Auth.VerifyPassword(password)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if ok {
			cookie, err := h.Auth.LoginCookie(r, r.FormValue("trust") == "on")
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			http.SetCookie(w, cookie)
			next := strings.TrimSpace(r.FormValue("next"))
			if next == "" || !strings.HasPrefix(next, "/") || strings.HasPrefix(next, "//") {
				next = "/"
			}
			http.Redirect(w, r, next, http.StatusSeeOther)
			return
		}
		data["Error"] = "Invalid password"
		data["Next"] = r.FormValue("next")
	}
	tmpl := h.templates["login.html"]
	if tmpl == nil {
		http.Error(w, "Template not found: login.html", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/html; charset=utf-8")
	if err := tmpl.ExecuteTemplate(w, "login", data); err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
	}
}

func (h *Handlers) PasswordSettings(w http.ResponseWriter, r *http.Request) {
	if h.Auth == nil {
		http.NotFound(w, r)
		return
	}
	data := newData(r)
	data["_parseTime"] = time.Duration(0)
	if r.Method == http.MethodPost {
		_ = r.ParseForm()
		current := r.FormValue("current_password")
		next := r.FormValue("new_password")
		confirm := r.FormValue("confirm_password")
		switch {
		case next == "":
			data["Error"] = "New password is required"
		case next != confirm:
			data["Error"] = "New passwords do not match"
		default:
			ok, err := h.Auth.VerifyPassword(current)
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			if !ok {
				data["Error"] = "Current password is incorrect"
				break
			}
			if err := h.Auth.SetPassword(next); err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			cookie, err := h.Auth.LoginCookie(r, true)
			if err != nil {
				http.Error(w, err.Error(), http.StatusInternalServerError)
				return
			}
			http.SetCookie(w, cookie)
			data["Success"] = "Password changed"
		}
	}
	h.render(w, r, "settings/password.html", data)
}
