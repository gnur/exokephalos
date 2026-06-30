package importer

import (
	"crypto/sha256"
	"encoding/binary"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/gnur/exokephalos/internal/id"
	"github.com/gnur/exokephalos/internal/markdown"
	"gopkg.in/yaml.v3"
)

// Result tracks the outcome of an import operation.
type Result struct {
	Imported int
	Skipped  int
	Errors   []string
}

// Import recursively scans sourceDir for .md files and imports them into exoDir.
// Each file gets frontmatter with the given type, preserving existing values where possible.
// Files are placed in exoDir/<first-3-id-chars>/<id>.md
func Import(sourceDir, exoDir, typ string) Result {
	result := Result{}

	err := filepath.Walk(sourceDir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return err
		}

		// Skip directories
		if info.IsDir() {
			return nil
		}

		// Only process .md files
		if !strings.HasSuffix(strings.ToLower(path), ".md") {
			return nil
		}

		// Import this file
		imported, skipReason, importErr := importFile(path, exoDir, typ)
		if importErr != nil {
			result.Errors = append(result.Errors, fmt.Sprintf("%s: %v", path, importErr))
		} else if imported {
			result.Imported++
		} else {
			result.Skipped++
			if skipReason != "" {
				result.Errors = append(result.Errors, fmt.Sprintf("%s: %s", path, skipReason))
			}
		}

		return nil
	})

	if err != nil {
		result.Errors = append(result.Errors, fmt.Sprintf("walk error: %v", err))
	}

	return result
}

// importFile imports a single markdown file into exoDir.
// Returns (imported, skipReason, error).
func importFile(sourcePath, exoDir, typ string) (bool, string, error) {
	// Read source file
	data, err := os.ReadFile(sourcePath)
	if err != nil {
		return false, "", fmt.Errorf("read file: %w", err)
	}

	// Parse frontmatter
	fm, body, err := markdown.ParseFrontmatterBytes(data)
	if err != nil {
		return false, "", fmt.Errorf("parse frontmatter: %w", err)
	}

	// Resolve fields by priority
	resolvedType := resolveType(fm, typ)
	resolvedTags := resolveTags(fm)
	resolvedCreated := resolveCreated(fm, sourcePath)
	resolvedID := resolveID(fm, resolvedCreated, sourcePath)
	resolvedTitle := resolveTitle(fm, body, sourcePath)

	// If the original file did not have a created date, but we kept a valid ID format,
	// extract the created date from the ID to keep the frontmatter consistent.
	if fm != nil {
		hasCreated := false
		if _, ok := fm["created"]; ok {
			hasCreated = true
		}
		if _, ok := fm["added"]; ok {
			hasCreated = true
		}
		if !hasCreated {
			if len(resolvedID) == 7 {
				days := id.DecodeBase32(resolvedID[:3])
				resolvedCreated = id.TimeFromDays(days)
			} else if len(resolvedID) == 9 {
				ts := id.DecodeBase62(resolvedID[:6])
				if ts >= 946684800 && ts <= 2524608000 {
					resolvedCreated = time.Unix(int64(ts), 0).UTC()
				}
			}
		}
	}

	// Build destination path containing the title if it is present
	destDir := filepath.Join(exoDir, resolvedID[:3])
	var fileName string
	if resolvedTitle != "" {
		slug := markdown.Slugify(resolvedTitle)
		if slug != "" {
			fileName = resolvedID + "-" + slug + ".md"
		} else {
			fileName = resolvedID + ".md"
		}
	} else {
		fileName = resolvedID + ".md"
	}
	destPath := filepath.Join(destDir, fileName)

	// Skip if destination exists
	if _, err := os.Stat(destPath); err == nil {
		return false, "destination already exists", nil
	}

	// Create destination directory
	if err := os.MkdirAll(destDir, 0755); err != nil {
		return false, "", fmt.Errorf("create directory: %w", err)
	}

	// Build new frontmatter, preserving all original fields
	newFM := make(map[string]interface{})
	if fm != nil {
		for k, v := range fm {
			newFM[k] = v
		}
	}
	newFM["type"] = resolvedType
	newFM["tags"] = resolvedTags
	newFM["created"] = resolvedCreated
	newFM["id"] = resolvedID
	newFM["title"] = resolvedTitle

	// Convert any timestamps to unquoted yaml.Node
	convertedFM := convertTimestamps(newFM).(map[string]interface{})

	// Write file
	if err := markdown.WriteFrontmatter(destPath, convertedFM, body); err != nil {
		return false, "", fmt.Errorf("write frontmatter: %w", err)
	}

	return true, "", nil
}

// resolveType returns fm["type"] if present, otherwise the provided type.
func resolveType(fm map[string]interface{}, typ string) string {
	if fm != nil {
		if t, ok := fm["type"].(string); ok && t != "" {
			return t
		}
	}
	return typ
}

// resolveTags returns fm["tags"] if present, otherwise empty slice.
func resolveTags(fm map[string]interface{}) []string {
	if fm != nil {
		if tags, ok := fm["tags"]; ok {
			switch t := tags.(type) {
			case []interface{}:
				result := make([]string, 0, len(t))
				for _, tag := range t {
					if s, ok := tag.(string); ok {
						result = append(result, s)
					}
				}
				return result
			case []string:
				return t
			}
		}
	}
	return []string{}
}

// resolveCreated returns fm["created"] or fm["added"] or file mod time.
func resolveCreated(fm map[string]interface{}, sourcePath string) time.Time {
	if fm != nil {
		// Try "created" first
		if val, ok := fm["created"]; ok && val != nil {
			if t, ok := val.(time.Time); ok {
				return t
			}
			if s, ok := val.(string); ok && s != "" {
				if t, err := parseDate(s); err == nil {
					return t
				}
			}
		}

		// Try "added"
		if val, ok := fm["added"]; ok && val != nil {
			if t, ok := val.(time.Time); ok {
				return t
			}
			if s, ok := val.(string); ok && s != "" {
				if t, err := parseDate(s); err == nil {
					return t
				}
			}
		}
	}

	// Fall back to file mod time
	if info, err := os.Stat(sourcePath); err == nil {
		return info.ModTime()
	}

	return time.Now()
}

// resolveID returns fm["id"] if it's in the new format (matching the created time),
// otherwise generates a deterministic one.
func resolveID(fm map[string]interface{}, created time.Time, sourcePath string) string {
	if fm != nil {
		if val, ok := fm["id"]; ok && val != nil {
			var idStr string
			switch v := val.(type) {
			case string:
				idStr = v
			case int:
				idStr = strconv.Itoa(v)
			case int64:
				idStr = strconv.FormatInt(v, 10)
			}
			
			// Check if the original frontmatter has a "created" or "added" field
			hasCreatedField := false
			if cVal, ok := fm["created"]; ok && cVal != nil {
				hasCreatedField = true
			}
			if !hasCreatedField {
				if aVal, ok := fm["added"]; ok && aVal != nil {
					hasCreatedField = true
				}
			}

			if hasNewIDFormat(idStr, created, hasCreatedField) {
				return idStr
			}
		}
	}
	// Generate deterministic ID based on created time and source path
	return generateDeterministicID(created, sourcePath)
}

// hasNewIDFormat checks if the ID matches the exo ID format.
// Supports both the 7-character lowercase base32 format and the 9-character legacy base62 format.
func hasNewIDFormat(idStr string, created time.Time, hasCreatedField bool) bool {
	if len(idStr) == 7 {
		for _, c := range idStr {
			if c == '0' {
				continue
			}
			if !((c >= 'a' && c <= 'z') || (c >= '2' && c <= '7')) {
				return false
			}
		}
		days := id.DecodeBase32(idStr[:3])
		t := id.TimeFromDays(days)
		
		if !hasCreatedField {
			year := t.Year()
			return year >= 2000 && year <= 2050
		}
		
		diff := t.Sub(created).Hours() / 24
		if diff < 0 {
			diff = -diff
		}
		return diff <= 30
	}

	if len(idStr) == 9 {
		for _, c := range idStr {
			if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z')) {
				return false
			}
		}
		ts := id.DecodeBase62(idStr[:6])
		if !hasCreatedField {
			return ts >= 946684800 && ts <= 2524608000
		}
		diff := float64(ts) - float64(created.Unix())
		if diff < 0 {
			diff = -diff
		}
		const thirtyDaysInSeconds = 30 * 24 * 60 * 60
		return diff <= thirtyDaysInSeconds
	}

	return false
}

// generateDeterministicID creates a deterministic ID from timestamp and source path.
func generateDeterministicID(created time.Time, sourcePath string) string {
	// Hash the source path to get deterministic random chars
	hash := sha256.Sum256([]byte(sourcePath))
	
	// Use first 8 bytes of hash as uint64 for random component
	seed := binary.BigEndian.Uint64(hash[:8])
	
	days := id.DaysSinceEpoch(created)
	encoded := id.EncodeBase32(days)
	
	// Generate 4 deterministic random base32 chars from the hash
	var randomPart []byte
	n := seed
	for len(randomPart) < 4 {
		randomPart = append(randomPart, id.Base32Chars[n%32])
		n /= 32
	}
	
	idStr := encoded + string(randomPart)
	for len(idStr) < 7 {
		idStr = "0" + idStr
	}
	
	return idStr
}

// deterministicRandomChars generates n random alphanumeric characters from a seed.
func deterministicRandomChars(seed uint64, n int) string {
	const base62Chars = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ"
	result := make([]byte, n)
	temp := seed
	for i := range result {
		result[i] = base62Chars[temp%62]
		temp /= 62
	}
	return string(result)
}

// resolveTitle returns fm["title"] or first # Header or filename.
func resolveTitle(fm map[string]interface{}, body, sourcePath string) string {
	if fm != nil {
		if title, ok := fm["title"].(string); ok && title != "" {
			return title
		}
	}

	// Try to extract first # Header from body
	if title := extractFirstHeader(body); title != "" {
		return title
	}

	// Fall back to filename without .md
	base := filepath.Base(sourcePath)
	return strings.TrimSuffix(base, filepath.Ext(base))
}

// extractFirstHeader finds the first # Header in markdown content.
func extractFirstHeader(body string) string {
	headerRegex := regexp.MustCompile(`(?m)^#\s+(.+)$`)
	matches := headerRegex.FindStringSubmatch(body)
	if len(matches) >= 2 {
		return strings.TrimSpace(matches[1])
	}
	return ""
}

// parseDate tries multiple date formats.
func parseDate(s string) (time.Time, error) {
	formats := []string{
		"2006-01-02",
		"2006-01-02T15:04:05Z07:00",
		"2006-01-02T15:04:05",
		"2006-01-02 15:04:05",
		time.RFC3339,
	}

	for _, format := range formats {
		if t, err := time.Parse(format, s); err == nil {
			return t, nil
		}
	}

	return time.Time{}, fmt.Errorf("unable to parse date: %s", s)
}

// convertTimestamps recursively converts any strings or time.Times that represent
// a valid timestamp to a yaml.Node with the !!timestamp tag, ensuring they are encoded unquoted.
func convertTimestamps(val interface{}) interface{} {
	switch v := val.(type) {
	case time.Time:
		return convertToTimestampNode(v)
	case string:
		if t, err := parseDate(v); err == nil {
			return convertToTimestampNode(t)
		}
		return v
	case map[string]interface{}:
		res := make(map[string]interface{}, len(v))
		for k, val := range v {
			res[k] = convertTimestamps(val)
		}
		return res
	case []interface{}:
		res := make([]interface{}, len(v))
		for i, val := range v {
			res[i] = convertTimestamps(val)
		}
		return res
	default:
		return v
	}
}

// convertToTimestampNode wraps a time.Time in a yaml.Node to serialize it as an unquoted scalar.
func convertToTimestampNode(t time.Time) *yaml.Node {
	return &yaml.Node{
		Kind:  yaml.ScalarNode,
		Tag:   "!!timestamp",
		Value: t.Format(time.RFC3339),
	}
}



