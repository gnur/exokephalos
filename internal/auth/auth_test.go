package auth

import (
	"net/http/httptest"
	"path/filepath"
	"testing"
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
