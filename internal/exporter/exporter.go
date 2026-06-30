package exporter

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/gnur/exokephalos/internal/cache"
	"github.com/gnur/exokephalos/internal/markdown"
)

// ExportOptions configures the export operation.
type ExportOptions struct {
	OutputDir  string
	TargetType string
}

// Result tracks the outcome of the export operation.
type Result struct {
	Exported int
	Errors   []string
}

// Export exports items from the cache to the target directory.
func Export(c *cache.Cache, opts ExportOptions) Result {
	res := Result{}

	if opts.OutputDir == "" {
		res.Errors = append(res.Errors, "output directory is required")
		return res
	}

	items, err := c.All()
	if err != nil {
		res.Errors = append(res.Errors, fmt.Sprintf("failed to get items from cache: %v", err))
		return res
	}

	for _, item := range items {
		// Filter by type if target type is specified
		if opts.TargetType != "" && item.Type != opts.TargetType {
			continue
		}

		// Calculate directories
		year := item.Created.Format("2006")
		month := item.Created.Format("01")

		title := item.Title("title")
		slug := markdown.Slugify(title)
		if slug == "" {
			slug = "untitled"
		}

		// Filter out exo specific frontmatter fields: id, type, created
		newFM := make(map[string]interface{})
		for k, v := range item.Frontmatter {
			if k == "id" || k == "type" || k == "created" {
				continue
			}
			newFM[k] = v
		}

		// Build target filename and handle conflicts
		destDir := filepath.Join(opts.OutputDir, item.Type, year, month)
		destPath := filepath.Join(destDir, slug+".md")

		if _, err := os.Stat(destPath); err == nil {
			for i := 1; ; i++ {
				candidate := filepath.Join(destDir, fmt.Sprintf("%s-%d.md", slug, i))
				if _, err := os.Stat(candidate); os.IsNotExist(err) {
					destPath = candidate
					break
				}
			}
		}

		// Ensure parent directory exists
		if err := os.MkdirAll(destDir, 0755); err != nil {
			res.Errors = append(res.Errors, fmt.Sprintf("failed to create directory %s: %v", destDir, err))
			continue
		}

		// Write exported item
		if err := markdown.WriteFrontmatter(destPath, newFM, item.Body); err != nil {
			res.Errors = append(res.Errors, fmt.Sprintf("failed to write export to %s: %v", destPath, err))
			continue
		}

		res.Exported++
	}

	return res
}
