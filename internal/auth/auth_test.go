package auth

import (
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestPasswordAndSessionFlow(t *testing.T) {
	m, err := New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer m.Close()

	if err := m.SetPassword("correct horse battery staple"); err != nil {
		t.Fatalf("SetPassword: %v", err)
	}
	ok, err := m.VerifyPassword("wrong")
	if err != nil {
		t.Fatalf("VerifyPassword wrong: %v", err)
	}
	if ok {
		t.Fatal("wrong password verified")
	}
	ok, err = m.VerifyPassword("correct horse battery staple")
	if err != nil {
		t.Fatalf("VerifyPassword correct: %v", err)
	}
	if !ok {
		t.Fatal("correct password did not verify")
	}

	req := httptest.NewRequest("POST", "http://example.com/login", nil)
	cookie, err := m.LoginCookie(req, false)
	if err != nil {
		t.Fatalf("LoginCookie: %v", err)
	}
	req = httptest.NewRequest("GET", "http://example.com/", nil)
	req.AddCookie(cookie)
	if !m.ValidRequest(req) {
		t.Fatal("session cookie was not accepted")
	}

	trusted, err := m.LoginCookie(req, true)
	if err != nil {
		t.Fatalf("trusted LoginCookie: %v", err)
	}
	if trusted.MaxAge <= 0 || trusted.Expires.IsZero() {
		t.Fatal("trusted cookie should be persistent")
	}
}

func TestAPIKeyLifecycle(t *testing.T) {
	m, err := New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer m.Close()

	raw, key, err := m.CreateAPIKey("test app", `type == "note"`, time.Now().Add(24*time.Hour))
	if err != nil {
		t.Fatalf("CreateAPIKey: %v", err)
	}
	if !strings.HasPrefix(raw, "exo_") {
		t.Fatalf("raw key = %q, want exo_ prefix", raw)
	}
	if key.AppName != "test app" || key.Filter != `type == "note"` {
		t.Fatalf("created key metadata = %+v", key)
	}

	verified, ok, err := m.VerifyAPIKey(raw)
	if err != nil {
		t.Fatalf("VerifyAPIKey: %v", err)
	}
	if !ok || verified == nil {
		t.Fatal("valid key did not verify")
	}
	if verified.LastUsedAt == "" {
		t.Fatal("last_used_at was not set")
	}

	keys, err := m.ListAPIKeys()
	if err != nil {
		t.Fatalf("ListAPIKeys: %v", err)
	}
	if len(keys) != 1 {
		t.Fatalf("keys length = %d, want 1", len(keys))
	}
	if strings.Contains(raw, keys[0].KeySuffix) && raw == keys[0].KeySuffix {
		t.Fatal("listed key exposed the raw key")
	}

	if err := m.RevokeAPIKey(key.ID); err != nil {
		t.Fatalf("RevokeAPIKey: %v", err)
	}
	if _, ok, err := m.VerifyAPIKey(raw); err != nil || ok {
		t.Fatalf("revoked key verify ok=%v err=%v, want false nil", ok, err)
	}
}

func TestCreateAPIKeyValidation(t *testing.T) {
	m, err := New(filepath.Join(t.TempDir(), "auth.sqlite"))
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	defer m.Close()

	if _, _, err := m.CreateAPIKey("", `true`, time.Now().Add(time.Hour)); err == nil {
		t.Fatal("missing app name should fail")
	}
	if _, _, err := m.CreateAPIKey("app", `not valid !!!`, time.Now().Add(time.Hour)); err == nil {
		t.Fatal("invalid CEL should fail")
	}
	if _, _, err := m.CreateAPIKey("app", `true`, time.Now().AddDate(1, 0, 1)); err == nil {
		t.Fatal("expiration beyond one year should fail")
	}
	if _, _, err := m.CreateAPIKey("app", `true`, time.Now().Add(-time.Hour)); err == nil {
		t.Fatal("past expiration should fail")
	}
}
