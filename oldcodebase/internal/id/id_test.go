package id

import (
	"strings"
	"testing"
	"time"
)

func TestEncodeBase32(t *testing.T) {
	tests := []struct {
		input    uint64
		expected string
	}{
		{0, "a"},
		{1, "b"},
		{31, "7"},
		{32, "ba"},
		{100, "de"},
		{1000, "7i"},
	}

	for _, tt := range tests {
		result := EncodeBase32(tt.input)
		if result != tt.expected {
			t.Errorf("EncodeBase32(%d) = %s, want %s", tt.input, result, tt.expected)
		}
		
		decoded := DecodeBase32(result)
		if decoded != tt.input {
			t.Errorf("DecodeBase32(%s) = %d, want %d", result, decoded, tt.input)
		}
	}
}

func TestEncodeBase62(t *testing.T) {
	tests := []struct {
		input    uint64
		expected string
	}{
		{0, "0"},
		{1, "1"},
		{10, "a"},
		{35, "z"},
		{36, "A"},
		{61, "Z"},
		{62, "10"},
	}

	for _, tt := range tests {
		result := EncodeBase62(tt.input)
		if result != tt.expected {
			t.Errorf("EncodeBase62(%d) = %s, want %s", tt.input, result, tt.expected)
		}
		
		decoded := DecodeBase62(result)
		if decoded != tt.input {
			t.Errorf("DecodeBase62(%s) = %d, want %d", result, decoded, tt.input)
		}
	}
}

func TestGenerateID(t *testing.T) {
	id := GenerateID()
	
	// Should be exactly 7 characters
	if len(id) != 7 {
		t.Errorf("GenerateID() length = %d, want 7", len(id))
	}
	
	// Should only contain valid base32 characters or '0'
	for _, c := range id {
		if c == '0' {
			continue
		}
		if !isValidBase32Char(byte(c)) {
			t.Errorf("GenerateID() contains invalid character: %c", c)
		}
	}
}

func TestGenerateIDFromTime(t *testing.T) {
	// Test with a known date: 2026-06-30
	testTime := time.Date(2026, 6, 30, 12, 0, 0, 0, time.UTC)
	idVal := GenerateIDFromTime(testTime)
	
	// Should be exactly 7 characters
	if len(idVal) != 7 {
		t.Errorf("GenerateIDFromTime() length = %d, want 7", len(idVal))
	}
	
	// Extract days part
	daysEncoded := idVal[:3]
	daysDecoded := DecodeBase32(daysEncoded)
	expectedDays := DaysSinceEpoch(testTime)
	if daysDecoded != expectedDays {
		t.Errorf("expected %d days encoded, got %d from prefix %s", expectedDays, daysDecoded, daysEncoded)
	}
	
	// Test padding for old date: 1989-01-18 (1 day since epoch -> prefix "b")
	oldTime := time.Date(1989, 1, 18, 0, 0, 0, 0, time.UTC)
	oldID := GenerateIDFromTime(oldTime)
	if len(oldID) != 7 {
		t.Errorf("GenerateIDFromTime(old) length = %d, want 7", len(oldID))
	}
	if !strings.HasPrefix(oldID, "00") {
		t.Errorf("GenerateIDFromTime(old) = %s, expected to start with 00 due to padding", oldID)
	}
}

func TestIsValidID(t *testing.T) {
	tests := []struct {
		input    string
		expected bool
	}{
		{"abcdefg", true},
		{"0abc234", true},
		{"abcde1g", false}, // '1' is not in Base32 alphabet
		{"abcdeFg", false}, // uppercase not allowed in 7-char format
		{"123456789", true}, // legacy 9-char format (valid)
		{"ABCDEFGHI", true}, // legacy 9-char format (valid)
		{"abcdefgh", false}, // length 8 not valid
	}

	for _, tt := range tests {
		result := IsValidID(tt.input)
		if result != tt.expected {
			t.Errorf("IsValidID(%q) = %t, want %t", tt.input, result, tt.expected)
		}
	}
}
