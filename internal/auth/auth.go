package auth

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"database/sql"
	"encoding/base32"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"golang.org/x/crypto/argon2"
	_ "modernc.org/sqlite"
)

const CookieName = "exo_auth"

const (
	argonTime    uint32 = 3
	argonMemory  uint32 = 64 * 1024
	argonThreads uint8  = 1
	argonKeyLen  uint32 = 32
)

type Manager struct {
	db *sql.DB
}

func New(dbPath string) (*Manager, error) {
	if err := os.MkdirAll(filepath.Dir(dbPath), 0700); err != nil {
		return nil, err
	}
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, err
	}
	m := &Manager{db: db}
	if err := m.migrate(); err != nil {
		_ = db.Close()
		return nil, err
	}
	return m, nil
}

func (m *Manager) Close() error {
	return m.db.Close()
}

func (m *Manager) EnsurePassword() error {
	hash, err := m.setting("password_hash")
	if err != nil {
		return err
	}
	if hash != "" {
		return nil
	}
	password, err := randomBase32(20)
	if err != nil {
		return err
	}
	hash, err = HashPassword(password)
	if err != nil {
		return err
	}
	if err := m.setSetting("password_hash", hash); err != nil {
		return err
	}
	fmt.Printf("exokephalos initial web password: %s\n", password)
	return nil
}

func (m *Manager) VerifyPassword(password string) (bool, error) {
	hash, err := m.setting("password_hash")
	if err != nil {
		return false, err
	}
	if hash == "" {
		return false, nil
	}
	return VerifyPassword(password, hash)
}

func (m *Manager) SetPassword(password string) error {
	hash, err := HashPassword(password)
	if err != nil {
		return err
	}
	if err := m.setSetting("password_hash", hash); err != nil {
		return err
	}
	_, err = m.db.Exec(`DELETE FROM auth_sessions`)
	return err
}

func (m *Manager) LoginCookie(r *http.Request, trust bool) (*http.Cookie, error) {
	token, err := randomToken(32)
	if err != nil {
		return nil, err
	}
	now := time.Now().UTC()
	expires := now.Add(12 * time.Hour)
	maxAge := 0
	if trust {
		expires = now.Add(30 * 24 * time.Hour)
		maxAge = int((30 * 24 * time.Hour).Seconds())
	}
	if _, err := m.db.Exec(`INSERT INTO auth_sessions(token_hash, created_at, expires_at) VALUES(?, ?, ?)`, tokenHash(token), now.Format(time.RFC3339Nano), expires.Format(time.RFC3339Nano)); err != nil {
		return nil, err
	}
	cookie := &http.Cookie{
		Name:     CookieName,
		Value:    token,
		Path:     "/",
		HttpOnly: true,
		SameSite: http.SameSiteLaxMode,
		MaxAge:   maxAge,
		Secure:   requestIsHTTPS(r),
	}
	if trust {
		cookie.Expires = expires
	}
	return cookie, nil
}

func (m *Manager) ValidRequest(r *http.Request) bool {
	cookie, err := r.Cookie(CookieName)
	if err != nil || cookie.Value == "" {
		return false
	}
	var expires string
	err = m.db.QueryRow(`SELECT expires_at FROM auth_sessions WHERE token_hash = ?`, tokenHash(cookie.Value)).Scan(&expires)
	if err != nil {
		return false
	}
	expiresAt, err := time.Parse(time.RFC3339Nano, expires)
	if err != nil || time.Now().UTC().After(expiresAt) {
		_, _ = m.db.Exec(`DELETE FROM auth_sessions WHERE token_hash = ?`, tokenHash(cookie.Value))
		return false
	}
	return true
}

func (m *Manager) Middleware(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if m.Exempt(r) || m.ValidRequest(r) {
			next.ServeHTTP(w, r)
			return
		}
		if strings.HasPrefix(r.URL.Path, "/api/") {
			http.Error(w, "authentication required", http.StatusUnauthorized)
			return
		}
		target := "/login"
		if r.URL.RequestURI() != "" && r.URL.Path != "/" {
			target += "?next=" + url.QueryEscape(r.URL.RequestURI())
		}
		http.Redirect(w, r, target, http.StatusSeeOther)
	})
}

func (m *Manager) Exempt(r *http.Request) bool {
	return r.URL.Path == "/login" ||
		r.URL.Path == "/ping" ||
		strings.HasPrefix(r.URL.Path, "/static/") ||
		strings.HasPrefix(r.URL.Path, "/assets/") ||
		strings.HasPrefix(r.URL.Path, "/icons/") ||
		r.URL.Path == "/manifest.webmanifest" ||
		r.URL.Path == "/registerSW.js" ||
		r.URL.Path == "/sw.js" ||
		strings.HasPrefix(r.URL.Path, "/workbox-") ||
		strings.HasPrefix(r.URL.Path, "/api/sync/")
}

func HashPassword(password string) (string, error) {
	salt := make([]byte, 16)
	if _, err := rand.Read(salt); err != nil {
		return "", err
	}
	key := argon2.IDKey([]byte(password), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return fmt.Sprintf("$argon2id$v=19$m=%d,t=%d,p=%d$%s$%s",
		argonMemory,
		argonTime,
		argonThreads,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(key),
	), nil
}

func VerifyPassword(password, encoded string) (bool, error) {
	parts := strings.Split(encoded, "$")
	if len(parts) != 6 || parts[1] != "argon2id" || parts[2] != "v=19" {
		return false, fmt.Errorf("unsupported password hash")
	}
	params := strings.Split(parts[3], ",")
	if len(params) != 3 {
		return false, fmt.Errorf("invalid password hash params")
	}
	memory, err := parseParam(params[0], "m")
	if err != nil {
		return false, err
	}
	timeCost, err := parseParam(params[1], "t")
	if err != nil {
		return false, err
	}
	parallelism, err := parseParam(params[2], "p")
	if err != nil {
		return false, err
	}
	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return false, err
	}
	expected, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return false, err
	}
	actual := argon2.IDKey([]byte(password), salt, uint32(timeCost), uint32(memory), uint8(parallelism), uint32(len(expected)))
	return subtle.ConstantTimeCompare(actual, expected) == 1, nil
}

func (m *Manager) migrate() error {
	stmts := []string{
		`CREATE TABLE IF NOT EXISTS auth_settings (
			key TEXT PRIMARY KEY,
			value TEXT NOT NULL
		)`,
		`CREATE TABLE IF NOT EXISTS auth_sessions (
			token_hash TEXT PRIMARY KEY,
			created_at TEXT NOT NULL,
			expires_at TEXT NOT NULL
		)`,
	}
	for _, stmt := range stmts {
		if _, err := m.db.Exec(stmt); err != nil {
			return err
		}
	}
	return nil
}

func (m *Manager) setting(key string) (string, error) {
	var value string
	err := m.db.QueryRow(`SELECT value FROM auth_settings WHERE key = ?`, key).Scan(&value)
	if err == sql.ErrNoRows {
		return "", nil
	}
	return value, err
}

func (m *Manager) setSetting(key, value string) error {
	_, err := m.db.Exec(`INSERT INTO auth_settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value`, key, value)
	return err
}

func parseParam(value, key string) (int, error) {
	prefix := key + "="
	if !strings.HasPrefix(value, prefix) {
		return 0, fmt.Errorf("missing %s param", key)
	}
	return strconv.Atoi(strings.TrimPrefix(value, prefix))
}

func randomBase32(length int) (string, error) {
	buf := make([]byte, length)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	encoded := base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(buf)
	encoded = strings.ToLower(encoded)
	if len(encoded) < length {
		return "", fmt.Errorf("short random password")
	}
	return encoded[:length], nil
}

func randomToken(length int) (string, error) {
	buf := make([]byte, length)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(buf), nil
}

func tokenHash(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

func requestIsHTTPS(r *http.Request) bool {
	return r.TLS != nil || strings.EqualFold(r.Header.Get("X-Forwarded-Proto"), "https")
}
