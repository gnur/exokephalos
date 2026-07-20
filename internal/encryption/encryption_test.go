package encryption

import "testing"

func TestRoundTripAndAAD(t *testing.T) {
	b, err := Encrypt("note1", "pass", "secret")
	if err != nil {
		t.Fatal(err)
	}
	got, err := Decrypt("note1", "pass", b)
	if err != nil || got != "secret" {
		t.Fatalf("%q %v", got, err)
	}
	if _, err := Decrypt("note2", "pass", b); err == nil {
		t.Fatal("expected AAD failure")
	}
}
