package id

import (
	"math/rand"
	"strings"
	"time"
)

// Base32Chars is the custom lowercase base32 alphabet.
const Base32Chars = "abcdefghijklmnopqrstuvwxyz234567"

// Legacy base62 chars.
const base62Chars = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"

// Epoch is the start date for day calculation: 1989-01-17.
var epoch = time.Date(1989, 1, 17, 0, 0, 0, 0, time.UTC)

// DaysSinceEpoch returns the number of days between epoch and t.
func DaysSinceEpoch(t time.Time) uint64 {
	duration := t.UTC().Sub(epoch)
	days := duration.Hours() / 24
	if days < 0 {
		return 0
	}
	return uint64(days)
}

// TimeFromDays returns the time.Time corresponding to days since epoch.
func TimeFromDays(days uint64) time.Time {
	return epoch.Add(time.Duration(days) * 24 * time.Hour)
}

// EncodeBase32 converts a uint64 to a base32 string.
func EncodeBase32(n uint64) string {
	if n == 0 {
		return "a"
	}

	var result []byte
	for n > 0 {
		result = append([]byte{Base32Chars[n%32]}, result...)
		n /= 32
	}
	return string(result)
}

// DecodeBase32 converts a base32 string back to a uint64.
func DecodeBase32(s string) uint64 {
	s = strings.TrimLeft(s, "0")
	var result uint64
	for _, c := range s {
		result *= 32
		idx := strings.IndexByte(Base32Chars, byte(c))
		if idx == -1 {
			continue
		}
		result += uint64(idx)
	}
	return result
}

// EncodeBase62 converts a uint64 to a base62 string (legacy).
func EncodeBase62(n uint64) string {
	if n == 0 {
		return "0"
	}

	var result []byte
	for n > 0 {
		result = append([]byte{base62Chars[n%62]}, result...)
		n /= 62
	}
	return string(result)
}

// DecodeBase62 converts a base62 string back to a uint64 (legacy).
func DecodeBase62(s string) uint64 {
	var result uint64
	for _, c := range s {
		result *= 62
		idx := strings.IndexByte(base62Chars, byte(c))
		if idx == -1 {
			continue
		}
		result += uint64(idx)
	}
	return result
}

// randomBase32Chars generates n random alphanumeric base32 characters.
func randomBase32Chars(n int) string {
	result := make([]byte, n)
	for i := range result {
		result[i] = Base32Chars[rand.Intn(32)]
	}
	return string(result)
}

// GenerateID creates a new 7-character lowercase base32 ID from the current timestamp.
func GenerateID() string {
	return GenerateIDFromTime(time.Now())
}

// GenerateIDFromTime creates a new 7-character lowercase base32 ID from the given timestamp.
// Format: [base32(days_since_1989-01-17)][4_random_chars], padded to 7 chars with zeroes.
func GenerateIDFromTime(t time.Time) string {
	days := DaysSinceEpoch(t)
	encoded := EncodeBase32(days)
	random := randomBase32Chars(4)

	id := encoded + random
	for len(id) < 7 {
		id = "0" + id
	}

	return id
}

// IsValidID checks if a string is a valid exo ID.
// Supports both the new 7-char lowercase base32 format and the legacy 9-char base62 format.
func IsValidID(s string) bool {
	if len(s) == 7 {
		for _, c := range s {
			if c == '0' {
				continue
			}
			if !isValidBase32Char(byte(c)) {
				return false
			}
		}
		return true
	}
	if len(s) == 9 {
		for _, c := range s {
			if !isValidBase62Char(byte(c)) {
				return false
			}
		}
		return true
	}
	return false
}

func isValidBase32Char(c byte) bool {
	return (c >= 'a' && c <= 'z') || (c >= '2' && c <= '7')
}

func isValidBase62Char(c byte) bool {
	return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')
}
