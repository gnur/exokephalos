// Package encryption implements the opaque encrypted note-body envelope.
package encryption

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"

	"golang.org/x/crypto/argon2"
)

const Prefix = "exo-encrypted:v1:"

type envelope struct {
	Version int    `json:"v"`
	KDF     string `json:"kdf"`
	Memory  uint32 `json:"m"`
	Time    uint32 `json:"t"`
	Lanes   uint8  `json:"p"`
	Salt    string `json:"s"`
	Nonce   string `json:"n"`
	Data    string `json:"ct"`
}

func IsEncrypted(body string) bool { return len(body) >= len(Prefix) && body[:len(Prefix)] == Prefix }

func Encrypt(noteID, passphrase, plaintext string) (string, error) {
	salt, nonce := make([]byte, 16), make([]byte, 12)
	if _, err := io.ReadFull(rand.Reader, salt); err != nil {
		return "", err
	}
	if _, err := io.ReadFull(rand.Reader, nonce); err != nil {
		return "", err
	}
	e := envelope{Version: 1, KDF: "argon2id", Memory: 65536, Time: 3, Lanes: 4, Salt: enc(salt), Nonce: enc(nonce)}
	key := argon2.IDKey([]byte(passphrase), salt, e.Time, e.Memory, e.Lanes, 32)
	defer clear(key)
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	e.Data = enc(gcm.Seal(nil, nonce, []byte(plaintext), aad(noteID)))
	b, err := json.Marshal(e)
	if err != nil {
		return "", err
	}
	return Prefix + enc(b), nil
}

func Decrypt(noteID, passphrase, body string) (string, error) {
	if !IsEncrypted(body) {
		return "", fmt.Errorf("not an encrypted note body")
	}
	b, err := dec(body[len(Prefix):])
	if err != nil {
		return "", fmt.Errorf("invalid envelope: %w", err)
	}
	var e envelope
	if err := json.Unmarshal(b, &e); err != nil || e.Version != 1 || e.KDF != "argon2id" || e.Memory == 0 || e.Time == 0 || e.Lanes == 0 {
		return "", fmt.Errorf("invalid encryption envelope")
	}
	salt, err := dec(e.Salt)
	if err != nil || len(salt) != 16 {
		return "", fmt.Errorf("invalid encryption salt")
	}
	nonce, err := dec(e.Nonce)
	if err != nil || len(nonce) != 12 {
		return "", fmt.Errorf("invalid encryption nonce")
	}
	ct, err := dec(e.Data)
	if err != nil {
		return "", fmt.Errorf("invalid ciphertext")
	}
	key := argon2.IDKey([]byte(passphrase), salt, e.Time, e.Memory, e.Lanes, 32)
	defer clear(key)
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", err
	}
	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	pt, err := gcm.Open(nil, nonce, ct, aad(noteID))
	if err != nil {
		return "", fmt.Errorf("unable to decrypt note: %w", err)
	}
	return string(pt), nil
}
func aad(id string) []byte         { return []byte("exo-encrypted:v1\x00" + id) }
func enc(b []byte) string          { return base64.RawURLEncoding.EncodeToString(b) }
func dec(s string) ([]byte, error) { return base64.RawURLEncoding.DecodeString(s) }
func clear(b []byte) {
	for i := range b {
		b[i] = 0
	}
}
