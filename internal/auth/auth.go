package auth

import (
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/subtle"
	"database/sql"
	"encoding/base32"
	"encoding/base64"
	"encoding/hex"
	"fmt"
	"math/big"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/filter"
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

const (
	apiKeyPrefix      = "exo_"
	apiKeyRandomBytes = 24
	base62Chars       = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
)

type contextKey string

const apiKeyContextKey contextKey = "api_key"

type Manager struct {
	db *sql.DB
}

type APIKey struct {
	ID         int64  `json:"id"`
	AppName    string `json:"app_name"`
	KeySuffix  string `json:"key_suffix"`
	Filter     string `json:"filter"`
	CreatedAt  string `json:"created_at"`
	ExpiresAt  string `json:"expires_at"`
	LastUsedAt string `json:"last_used_at"`
	RevokedAt  string `json:"revoked_at"`
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

func (m *Manager) EnsurePassword() (string, error) {
	hash, err := m.setting("password_hash")
	if err != nil {
		return "", err
	}
	if hash != "" {
		return "", nil
	}
	password, err := randomBase32(20)
	if err != nil {
		return "", err
	}
	hash, err = HashPassword(password)
	if err != nil {
		return "", err
	}
	if err := m.setSetting("password_hash", hash); err != nil {
		return "", err
	}
	return password, nil
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

func (m *Manager) ImportLegacy(dbPath string) error {
	if strings.TrimSpace(dbPath) == "" {
		return nil
	}
	if _, err := os.Stat(dbPath); err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	currentPassword, err := m.setting("password_hash")
	if err != nil {
		return err
	}
	if currentPassword != "" {
		return nil
	}
	legacy, err := New(dbPath)
	if err != nil {
		return err
	}
	defer legacy.Close()
	legacyPassword, err := legacy.setting("password_hash")
	if err != nil {
		return err
	}
	if legacyPassword == "" {
		return nil
	}
	if err := copyAuthSettings(legacy.db, m.db); err != nil {
		return err
	}
	if err := copyAuthSessions(legacy.db, m.db); err != nil {
		return err
	}
	return copyAPIKeys(legacy.db, m.db)
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
		if m.apiKeyEligible(r) {
			if raw := APIKeyFromRequest(r); raw != "" {
				key, ok, err := m.VerifyAPIKey(raw)
				if err != nil {
					http.Error(w, err.Error(), http.StatusInternalServerError)
					return
				}
				if ok {
					next.ServeHTTP(w, r.WithContext(context.WithValue(r.Context(), apiKeyContextKey, key)))
					return
				}
			}
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

func (m *Manager) apiKeyEligible(r *http.Request) bool {
	return r.Method == http.MethodGet && strings.HasPrefix(r.URL.Path, "/api/items/")
}

func (m *Manager) Exempt(r *http.Request) bool {
	return r.URL.Path == "/login" ||
		r.URL.Path == "/ping" ||
		r.URL.Path == "/healthz" ||
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
		`CREATE TABLE IF NOT EXISTS api_keys (
			id INTEGER PRIMARY KEY AUTOINCREMENT,
			app_name TEXT NOT NULL,
			key_hash TEXT NOT NULL UNIQUE,
			key_suffix TEXT NOT NULL,
			filter TEXT NOT NULL,
			created_at TEXT NOT NULL,
			expires_at TEXT NOT NULL,
			last_used_at TEXT NOT NULL DEFAULT '',
			revoked_at TEXT NOT NULL DEFAULT ''
		)`,
	}
	for _, stmt := range stmts {
		if _, err := m.db.Exec(stmt); err != nil {
			return err
		}
	}
	return nil
}

func copyAuthSettings(src, dst *sql.DB) error {
	rows, err := src.Query(`SELECT key, value FROM auth_settings`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var key, value string
		if err := rows.Scan(&key, &value); err != nil {
			return err
		}
		if _, err := dst.Exec(`INSERT INTO auth_settings(key, value) VALUES(?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value`, key, value); err != nil {
			return err
		}
	}
	return rows.Err()
}

func copyAuthSessions(src, dst *sql.DB) error {
	rows, err := src.Query(`SELECT token_hash, created_at, expires_at FROM auth_sessions`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var tokenHash, createdAt, expiresAt string
		if err := rows.Scan(&tokenHash, &createdAt, &expiresAt); err != nil {
			return err
		}
		if _, err := dst.Exec(`INSERT OR IGNORE INTO auth_sessions(token_hash, created_at, expires_at) VALUES(?, ?, ?)`, tokenHash, createdAt, expiresAt); err != nil {
			return err
		}
	}
	return rows.Err()
}

func copyAPIKeys(src, dst *sql.DB) error {
	rows, err := src.Query(`SELECT app_name, key_hash, key_suffix, filter, created_at, expires_at, last_used_at, revoked_at FROM api_keys`)
	if err != nil {
		return err
	}
	defer rows.Close()
	for rows.Next() {
		var key APIKey
		var keyHash string
		if err := rows.Scan(&key.AppName, &keyHash, &key.KeySuffix, &key.Filter, &key.CreatedAt, &key.ExpiresAt, &key.LastUsedAt, &key.RevokedAt); err != nil {
			return err
		}
		if _, err := dst.Exec(`INSERT OR IGNORE INTO api_keys(app_name, key_hash, key_suffix, filter, created_at, expires_at, last_used_at, revoked_at) VALUES(?, ?, ?, ?, ?, ?, ?, ?)`,
			key.AppName, keyHash, key.KeySuffix, key.Filter, key.CreatedAt, key.ExpiresAt, key.LastUsedAt, key.RevokedAt); err != nil {
			return err
		}
	}
	return rows.Err()
}

func (m *Manager) CreateAPIKey(appName, filterExpr string, expiresAt time.Time) (string, APIKey, error) {
	appName = strings.TrimSpace(appName)
	filterExpr = strings.TrimSpace(filterExpr)
	if appName == "" {
		return "", APIKey{}, fmt.Errorf("app name is required")
	}
	if filterExpr == "" {
		return "", APIKey{}, fmt.Errorf("filter is required")
	}
	if _, err := filter.Compile(filterExpr); err != nil {
		return "", APIKey{}, fmt.Errorf("invalid filter: %w", err)
	}
	now := time.Now().UTC()
	expiresAt = expiresAt.UTC()
	if !expiresAt.After(now) {
		return "", APIKey{}, fmt.Errorf("expiration date must be in the future")
	}
	maxExpiresAt := time.Date(now.Year()+1, now.Month(), now.Day(), 23, 59, 59, int(time.Second-time.Nanosecond), time.UTC)
	if expiresAt.After(maxExpiresAt) {
		return "", APIKey{}, fmt.Errorf("expiration date must be within one year")
	}
	raw, err := randomAPIKey()
	if err != nil {
		return "", APIKey{}, err
	}
	suffix := keySuffix(raw)
	res, err := m.db.Exec(`INSERT INTO api_keys(app_name, key_hash, key_suffix, filter, created_at, expires_at) VALUES(?, ?, ?, ?, ?, ?)`,
		appName, tokenHash(raw), suffix, filterExpr, now.Format(time.RFC3339Nano), expiresAt.Format(time.RFC3339Nano))
	if err != nil {
		return "", APIKey{}, err
	}
	id, _ := res.LastInsertId()
	key := APIKey{
		ID:        id,
		AppName:   appName,
		KeySuffix: suffix,
		Filter:    filterExpr,
		CreatedAt: now.Format(time.RFC3339Nano),
		ExpiresAt: expiresAt.Format(time.RFC3339Nano),
	}
	return raw, key, nil
}

func (m *Manager) ListAPIKeys() ([]APIKey, error) {
	rows, err := m.db.Query(`SELECT id, app_name, key_suffix, filter, created_at, expires_at, last_used_at, revoked_at FROM api_keys ORDER BY created_at DESC`)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	keys := []APIKey{}
	for rows.Next() {
		var key APIKey
		if err := rows.Scan(&key.ID, &key.AppName, &key.KeySuffix, &key.Filter, &key.CreatedAt, &key.ExpiresAt, &key.LastUsedAt, &key.RevokedAt); err != nil {
			return nil, err
		}
		keys = append(keys, key)
	}
	return keys, rows.Err()
}

func (m *Manager) RevokeAPIKey(id int64) error {
	_, err := m.db.Exec(`UPDATE api_keys SET revoked_at = ? WHERE id = ? AND revoked_at = ''`, time.Now().UTC().Format(time.RFC3339Nano), id)
	return err
}

func (m *Manager) VerifyAPIKey(raw string) (*APIKey, bool, error) {
	if !strings.HasPrefix(raw, apiKeyPrefix) {
		return nil, false, nil
	}
	var key APIKey
	var hash string
	err := m.db.QueryRow(`SELECT id, app_name, key_hash, key_suffix, filter, created_at, expires_at, last_used_at, revoked_at FROM api_keys WHERE key_hash = ?`, tokenHash(raw)).
		Scan(&key.ID, &key.AppName, &hash, &key.KeySuffix, &key.Filter, &key.CreatedAt, &key.ExpiresAt, &key.LastUsedAt, &key.RevokedAt)
	if err == sql.ErrNoRows {
		return nil, false, nil
	}
	if err != nil {
		return nil, false, err
	}
	if subtle.ConstantTimeCompare([]byte(hash), []byte(tokenHash(raw))) != 1 {
		return nil, false, nil
	}
	expiresAt, err := time.Parse(time.RFC3339Nano, key.ExpiresAt)
	if err != nil {
		return nil, false, err
	}
	if key.RevokedAt != "" || !expiresAt.After(time.Now().UTC()) {
		return nil, false, nil
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	if _, err := m.db.Exec(`UPDATE api_keys SET last_used_at = ? WHERE id = ?`, now, key.ID); err != nil {
		return nil, false, err
	}
	key.LastUsedAt = now
	return &key, true, nil
}

func APIKeyFromContext(ctx context.Context) (*APIKey, bool) {
	key, ok := ctx.Value(apiKeyContextKey).(*APIKey)
	return key, ok
}

func APIKeyFromRequest(r *http.Request) string {
	if authz := strings.TrimSpace(r.Header.Get("Authorization")); authz != "" {
		parts := strings.Fields(authz)
		if len(parts) == 2 && strings.EqualFold(parts[0], "Bearer") {
			return parts[1]
		}
	}
	return strings.TrimSpace(r.Header.Get("X-API-Key"))
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

func randomAPIKey() (string, error) {
	buf := make([]byte, apiKeyRandomBytes)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return apiKeyPrefix + encodeBase62(buf), nil
}

func encodeBase62(buf []byte) string {
	n := new(big.Int).SetBytes(buf)
	if n.Sign() == 0 {
		return "0"
	}
	base := big.NewInt(62)
	zero := big.NewInt(0)
	mod := new(big.Int)
	var out []byte
	for n.Cmp(zero) > 0 {
		n.DivMod(n, base, mod)
		out = append([]byte{base62Chars[mod.Int64()]}, out...)
	}
	return string(out)
}

func keySuffix(key string) string {
	if len(key) <= 8 {
		return key
	}
	return key[len(key)-8:]
}

func tokenHash(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

func requestIsHTTPS(r *http.Request) bool {
	return r.TLS != nil || strings.EqualFold(r.Header.Get("X-Forwarded-Proto"), "https")
}
